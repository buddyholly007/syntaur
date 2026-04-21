//! Z-Wave Serial API link layer — the SOF/ACK/NAK/CAN dance + retransmit.
//!
//! Spec summary (from `INS12350 Z-Wave Host Controller Interface`,
//! "Transfer Layer" section):
//!
//! - Sender transmits a Data Frame (SOF + ...) and waits for an **ACK
//!   (0x06)**. Budget is **1500 ms** from end of transmit.
//! - **NAK (0x15)** — checksum or framing error at the receiver.
//!   Retransmit the same Data Frame.
//! - **CAN (0x18)** — the receiver was in the middle of sending its
//!   own Data Frame when ours arrived; it won. We back off a spec-
//!   defined delay and retransmit.
//! - Retransmit **up to 3 times** total (4 attempts including the
//!   original). After that the link is considered dead.
//! - On successful receive of a Data Frame, we transmit an **ACK**.
//!   On checksum failure of an inbound Data Frame, we transmit a **NAK**.
//!
//! This module is generic over any `tokio::io::AsyncRead + AsyncWrite`
//! so unit tests drive it with `tokio_test::io::Builder` and the
//! real controller talks over `tokio-serial`.

use std::time::Duration;

use log::{debug, warn};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

use crate::frame::{self, Frame, ACK, CAN, NAK, SOF};

pub const ACK_TIMEOUT: Duration = Duration::from_millis(1500);
pub const CAN_BACKOFF: Duration = Duration::from_millis(100);
pub const MAX_RETRANSMITS: u8 = 3;

#[derive(Debug, Error)]
pub enum LinkError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ACK timeout after {} ms", ACK_TIMEOUT.as_millis())]
    AckTimeout,
    #[error("peer rejected {0} retransmits")]
    TooManyRetransmits(u8),
    #[error("unexpected control byte {0:#04x} while awaiting ACK")]
    UnexpectedControl(u8),
    #[error("peer closed the transport mid-frame")]
    Eof,
    #[error("frame parse failed: {0}")]
    FrameParse(#[from] frame::FrameParseError),
}

/// Link-layer state machine wrapping an AsyncRead+AsyncWrite transport.
///
/// One `LinkLayer` owns the transport and serializes access — callers
/// should hold it behind a mutex if multiple tasks need to send
/// concurrently. Incoming Data Frames are surfaced via `recv_frame`.
pub struct LinkLayer<T> {
    io: T,
}

impl<T> LinkLayer<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(io: T) -> Self {
        Self { io }
    }

    /// Send a Data Frame and wait for ACK, retransmitting on NAK/CAN.
    ///
    /// Returns Ok(()) after ACK arrives, Err(LinkError::TooManyRetransmits)
    /// when the peer keeps NAK'ing us, Err(LinkError::AckTimeout) when
    /// the peer never responds.
    pub async fn send_frame(&mut self, frame: &Frame) -> Result<(), LinkError> {
        let bytes = frame.encode();
        let mut attempt: u8 = 0;
        loop {
            self.io.write_all(&bytes).await?;
            self.io.flush().await?;
            debug!(
                "[zwave-link] tx attempt {} ({} bytes): {:02X?}",
                attempt + 1,
                bytes.len(),
                &bytes[..]
            );

            match timeout(ACK_TIMEOUT, self.read_control_byte()).await {
                Ok(Ok(ACK)) => return Ok(()),
                Ok(Ok(NAK)) => {
                    warn!("[zwave-link] peer NAK on attempt {}", attempt + 1);
                }
                Ok(Ok(CAN)) => {
                    warn!("[zwave-link] peer CAN on attempt {}, backing off", attempt + 1);
                    tokio::time::sleep(CAN_BACKOFF).await;
                }
                Ok(Ok(other)) => return Err(LinkError::UnexpectedControl(other)),
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(LinkError::AckTimeout),
            }

            attempt += 1;
            if attempt > MAX_RETRANSMITS {
                return Err(LinkError::TooManyRetransmits(attempt));
            }
        }
    }

    /// Block until a complete inbound Data Frame arrives, ACK'ing it
    /// on success and NAK'ing it on checksum/framing failure per spec.
    ///
    /// ACK / NAK / CAN control bytes from the peer are discarded here —
    /// the sender path (`send_frame`) owns them. That means `recv_frame`
    /// is strictly for the unsolicited-Data-Frame receive path, which
    /// is what higher layers (controller event loop) want.
    pub async fn recv_frame(&mut self) -> Result<Frame, LinkError> {
        loop {
            let first = self.read_byte().await?;
            match first {
                SOF => {
                    // After SOF + length, exactly `length` more bytes
                    // remain on the wire — `parse()` uses
                    // `total = 2 + length` for the full frame and we've
                    // already consumed the first two (SOF + length).
                    let length = self.read_byte().await?;
                    let remaining = length as usize;
                    let mut rest = vec![0u8; remaining];
                    self.io.read_exact(&mut rest).await?;

                    // Reassemble for the parser.
                    let mut whole = Vec::with_capacity(2 + remaining);
                    whole.push(SOF);
                    whole.push(length);
                    whole.extend_from_slice(&rest);

                    match frame::parse(&whole) {
                        Ok((f, _n)) => {
                            // ACK the frame.
                            self.io.write_all(&[ACK]).await?;
                            self.io.flush().await?;
                            return Ok(f);
                        }
                        Err(e) => {
                            warn!("[zwave-link] bad inbound frame, NAK: {:?}", e);
                            self.io.write_all(&[NAK]).await?;
                            self.io.flush().await?;
                            // Loop — spec says keep reading.
                        }
                    }
                }
                ACK | NAK | CAN => {
                    // Stray control byte outside of our own transmit
                    // window — discard. Could happen if a send_frame
                    // timed out and the peer ACK'd late.
                    debug!("[zwave-link] rx stray control {:#04x}", first);
                }
                other => {
                    warn!("[zwave-link] rx garbage {:#04x}, discarding", other);
                }
            }
        }
    }

    /// Read a single control byte (ACK/NAK/CAN) or the first byte of
    /// an inbound Data Frame. Loops past any leading garbage — spec
    /// allows that after a bus reset.
    async fn read_control_byte(&mut self) -> Result<u8, LinkError> {
        loop {
            let b = self.read_byte().await?;
            match b {
                ACK | NAK | CAN => return Ok(b),
                // A SOF here means the peer sent us a Data Frame while
                // we were waiting for an ACK — spec allows full-duplex,
                // but for the simple sender state machine we want the
                // control byte. Drain the rest of the frame and loop.
                SOF => {
                    let length = self.read_byte().await?;
                    let remaining = length as usize;
                    let mut skip = vec![0u8; remaining];
                    self.io.read_exact(&mut skip).await?;
                    debug!("[zwave-link] drained inbound SOF during ACK wait");
                }
                _ => {
                    debug!("[zwave-link] skipping garbage {:#04x} while awaiting ACK", b);
                }
            }
        }
    }

    async fn read_byte(&mut self) -> Result<u8, LinkError> {
        let mut buf = [0u8; 1];
        let n = self.io.read(&mut buf).await?;
        if n == 0 {
            return Err(LinkError::Eof);
        }
        Ok(buf[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Frame, FrameKind};

    /// Happy path: we send GetCapabilities (0x07), peer ACKs.
    #[tokio::test]
    async fn send_frame_receives_ack() {
        let want_on_wire: &[u8] = &[0x01, 0x03, 0x00, 0x07, 0xFB];
        let io = tokio_test::io::Builder::new()
            .write(want_on_wire)
            .read(&[ACK])
            .build();
        let mut link = LinkLayer::new(io);
        link.send_frame(&Frame::request(0x07, vec![]))
            .await
            .expect("send_frame");
    }

    /// NAK on first attempt → retransmit → ACK. Spec-mandated behavior.
    #[tokio::test]
    async fn send_frame_retransmits_on_nak() {
        let wire: &[u8] = &[0x01, 0x03, 0x00, 0x07, 0xFB];
        let io = tokio_test::io::Builder::new()
            .write(wire)
            .read(&[NAK])
            .write(wire)
            .read(&[ACK])
            .build();
        let mut link = LinkLayer::new(io);
        link.send_frame(&Frame::request(0x07, vec![])).await.unwrap();
    }

    /// CAN (collision) triggers backoff, then retransmit, then ACK.
    #[tokio::test(start_paused = true)]
    async fn send_frame_backs_off_on_can_then_succeeds() {
        let wire: &[u8] = &[0x01, 0x03, 0x00, 0x07, 0xFB];
        let io = tokio_test::io::Builder::new()
            .write(wire)
            .read(&[CAN])
            .wait(CAN_BACKOFF)
            .write(wire)
            .read(&[ACK])
            .build();
        let mut link = LinkLayer::new(io);
        link.send_frame(&Frame::request(0x07, vec![])).await.unwrap();
    }

    /// Peer never ACKs → we time out after 1500 ms.
    #[tokio::test(start_paused = true)]
    async fn send_frame_times_out_without_ack() {
        let wire: &[u8] = &[0x01, 0x03, 0x00, 0x07, 0xFB];
        // Build an IO that accepts the write and then blocks on read
        // (wait advances virtual time past the ACK deadline).
        let io = tokio_test::io::Builder::new()
            .write(wire)
            .wait(Duration::from_millis(2000))
            .build();
        let mut link = LinkLayer::new(io);
        let err = link.send_frame(&Frame::request(0x07, vec![])).await.unwrap_err();
        assert!(matches!(err, LinkError::AckTimeout), "got {:?}", err);
    }

    /// Four NAKs in a row — spec limits us to 3 retransmits (4 total tries).
    #[tokio::test(start_paused = true)]
    async fn send_frame_gives_up_after_three_nak_retransmits() {
        let wire: &[u8] = &[0x01, 0x03, 0x00, 0x07, 0xFB];
        let mut builder = tokio_test::io::Builder::new();
        for _ in 0..4 {
            builder.write(wire).read(&[NAK]);
        }
        let io = builder.build();
        let mut link = LinkLayer::new(io);
        let err = link.send_frame(&Frame::request(0x07, vec![])).await.unwrap_err();
        assert!(matches!(err, LinkError::TooManyRetransmits(_)), "got {:?}", err);
    }

    /// recv_frame should ACK a well-formed inbound frame and return it.
    #[tokio::test]
    async fn recv_frame_acks_valid_frame() {
        // Peer sends a Response to GetCapabilities.
        // 0x01 len=0x03 type=0x01 func=0x07 cksum=0xFA (0xFF^0x03^0x01^0x07)
        let inbound = &[0x01, 0x03, 0x01, 0x07, 0xFA];
        let io = tokio_test::io::Builder::new()
            .read(inbound)
            .write(&[ACK])
            .build();
        let mut link = LinkLayer::new(io);
        let frame = link.recv_frame().await.expect("recv_frame");
        assert_eq!(frame.kind, FrameKind::Response);
        assert_eq!(frame.function, 0x07);
    }

    /// recv_frame should NAK a frame with a busted checksum, then keep
    /// reading until a good one arrives.
    #[tokio::test]
    async fn recv_frame_naks_bad_checksum_then_accepts_next() {
        let bad = &[0x01, 0x03, 0x01, 0x07, 0x00]; // wrong checksum
        let good = &[0x01, 0x03, 0x01, 0x07, 0xFA];
        let io = tokio_test::io::Builder::new()
            .read(bad)
            .write(&[NAK])
            .read(good)
            .write(&[ACK])
            .build();
        let mut link = LinkLayer::new(io);
        let frame = link.recv_frame().await.expect("recv_frame");
        assert_eq!(frame.function, 0x07);
    }
}
