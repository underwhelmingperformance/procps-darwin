// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Snapshot, SnapshotRequest};

/// The version of the protocol that this build uses.
///
/// Every message starts with its sender's version, and a receiver checks the
/// version before it decodes the body. postcard's encoding does not describe
/// itself, so a body from another version could decode into the wrong values.
/// Increase `VERSION` whenever the encoding of a [`Request`] or a [`Response`]
/// changes, including when a `procps-core` or `darwin-proc` type in them gains
/// a field or an enum variant. The `helper_protocol` tests compare fixed
/// messages with their encoding in the current version. The messages use every
/// enum variant, and within each struct, fields of the same type have different
/// values. The tests therefore fail when a field or variant is added, removed
/// or moved, but an enum variant added at the end leaves the fixed messages
/// unchanged.
///
/// ```
/// use procps_core::helper::{Refusal, Response, VERSION, WriteMessage};
///
/// let mut wire = Vec::new();
/// wire.write_message(&Response::Refused(Refusal::Version))?;
///
/// assert_eq!(wire[..2], VERSION.to_be_bytes());
/// # Ok::<(), procps_core::helper::ProtocolError>(())
/// ```
pub const VERSION: u16 = 3;

/// The size of a message's header, which contains the version and the length
/// of the body.
const HEADER: usize = 6;

/// A request from a tool to the helper.
///
/// ```
/// use procps_core::{SnapshotRequest, helper::Request};
///
/// let request = Request::Snapshot(SnapshotRequest::default().with_system());
///
/// assert!(matches!(request, Request::Snapshot(snapshot) if snapshot.wants_system()));
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Take a snapshot.
    Snapshot(SnapshotRequest),
}

/// The helper's reply to a [`Request`].
///
/// ```
/// use procps_core::helper::{Refusal, Response};
///
/// let response = Response::Refused(Refusal::Malformed);
///
/// assert!(matches!(response, Response::Refused(_)));
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    /// The snapshot that the request asked for.
    Snapshot(Box<Snapshot>),
    /// The helper did not handle the request.
    Refused(Refusal),
}

/// The reason why the helper did not handle a request.
///
/// [`HelperOrLocal`](crate::HelperOrLocal) reads process data in the tool after
/// any refusal or [`ProtocolError`].
///
/// When a request has another version or is too large, or when the helper has
/// too many connections open, the helper sends a refusal and closes the
/// connection without reading the rest of the request. A tool that is still
/// writing the request then gets [`io::ErrorKind::BrokenPipe`] or
/// [`io::ErrorKind::NotConnected`] from the write, and can read the refusal
/// afterwards.
///
/// ```
/// use procps_core::helper::Refusal;
///
/// assert_eq!(Refusal::Busy.to_string(), "the helper is busy");
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
pub enum Refusal {
    /// The request has another protocol version. The header of this refusal
    /// has the helper's version, so a tool with another version detects the
    /// mismatch from the header and does not decode the body.
    #[error("the helper uses another version of the protocol")]
    Version,
    /// The request's body is larger than [`Request::LIMIT`].
    #[error("the request is too large for the helper")]
    RequestTooLarge,
    /// The helper could not decode the request.
    #[error("the helper could not decode the request")]
    Malformed,
    /// The helper could not take the snapshot, for example because it could
    /// not list the processes.
    #[error("the helper could not read the process data")]
    Failed,
    /// The helper did not finish the request within its time limit.
    #[error("the request took longer than the helper's time limit")]
    TimedOut,
    /// The snapshot's estimated size in memory, or the encoded response, is
    /// larger than [`Response::LIMIT`].
    #[error("the snapshot is too large for the helper to send")]
    ResponseTooLarge,
    /// The helper has too many connections open, for all clients or for the
    /// client's user, or the wait for a worker used more than half the
    /// snapshot time limit.
    #[error("the helper is busy")]
    Busy,
}

/// A message in the protocol: a [`Request`] or a [`Response`].
///
/// ```
/// use procps_core::helper::{Message, Request, Response};
///
/// assert!(Request::LIMIT < Response::LIMIT);
/// ```
pub trait Message: Serialize + DeserializeOwned + sealed::Sealed {
    /// The largest body, in bytes, that a peer sends or accepts.
    const LIMIT: u32;
}

impl Message for Request {
    /// Darwin's process IDs are below 100,000, so postcard encodes each one in
    /// at most three bytes. A request for every process takes less than
    /// 300 KiB.
    const LIMIT: u32 = 1 << 20;
}

impl Message for Response {
    /// A snapshot with every memory region of every process can take tens of
    /// megabytes, and 256 MiB leaves room for several times that.
    const LIMIT: u32 = 1 << 28;
}

mod sealed {
    pub trait Sealed {}

    impl Sealed for super::Request {}

    impl Sealed for super::Response {}
}

/// An error that prevents a message from being sent or received.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Reading from or writing to the connection failed. A read produces
    /// [`io::ErrorKind::UnexpectedEof`] when the connection closes before a
    /// message is complete, including before its first byte.
    #[error("cannot read from or write to the helper connection")]
    Io(#[from] io::Error),
    /// The peer uses another version of the protocol.
    #[error("the peer uses version {peer} of the helper protocol, not version {VERSION}")]
    Version {
        /// The peer's version.
        peer: u16,
    },
    /// The message's body is larger than its type's [`Message::LIMIT`].
    #[error("a helper message body of {length} bytes is larger than the limit of {limit} bytes")]
    TooLarge {
        /// The size of the body, in bytes.
        length: u64,
        /// The limit, in bytes.
        limit: u32,
    },
    /// The message could not be encoded, or its body could not be decoded.
    #[error("cannot encode or decode a helper message")]
    Encoding(#[from] postcard::Error),
    /// The body has bytes after the encoded value.
    #[error("a helper message body has {0} bytes after the encoded value")]
    TrailingBytes(usize),
}

/// Reads protocol messages.
///
/// A message has a header and a body. The header is the protocol version as a
/// big-endian `u16`, then the length of the body as a big-endian `u32`. The
/// body is the [`Request`] or [`Response`] encoded with postcard.
pub trait ReadMessage: Read {
    /// Reads one message of type `M`.
    ///
    /// The method stops at the first error. After a version or length error,
    /// the rest of the message remains unread. The body's buffer grows only as
    /// the bytes arrive, so a peer that announces a large body and then stops
    /// sending does not make the reader allocate the whole body.
    ///
    /// ```
    /// use std::io::Cursor;
    ///
    /// use procps_core::{
    ///     SnapshotRequest,
    ///     helper::{ReadMessage, Request, WriteMessage},
    /// };
    ///
    /// let request = Request::Snapshot(SnapshotRequest::default());
    /// let mut wire = Vec::new();
    /// wire.write_message(&request)?;
    ///
    /// assert_eq!(Cursor::new(wire).read_message::<Request>()?, request);
    /// # Ok::<(), procps_core::helper::ProtocolError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ProtocolError`] if the read fails or the connection closes
    /// before the message is complete, if the message has another version or a
    /// body larger than [`Message::LIMIT`], or if the body is not exactly one
    /// `M`.
    fn read_message<M: Message>(&mut self) -> Result<M, ProtocolError> {
        let mut version = [0; 2];
        self.read_exact(&mut version)?;
        let peer = u16::from_be_bytes(version);

        if peer != VERSION {
            return Err(ProtocolError::Version { peer });
        }

        let mut length = [0; 4];
        self.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length);

        if length > M::LIMIT {
            return Err(ProtocolError::TooLarge {
                length: u64::from(length),
                limit: M::LIMIT,
            });
        }

        let mut body = Vec::new();
        Read::take(&mut *self, u64::from(length)).read_to_end(&mut body)?;

        if u32::try_from(body.len()) != Ok(length) {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
        }

        let (message, rest) = postcard::take_from_bytes(&body)?;

        if !rest.is_empty() {
            return Err(ProtocolError::TrailingBytes(rest.len()));
        }

        Ok(message)
    }
}

impl<R: Read + ?Sized> ReadMessage for R {}

/// Writes protocol messages, in the format that [`ReadMessage`] describes.
pub trait WriteMessage: Write {
    /// Writes `message` with one `write_all` call, then flushes.
    ///
    /// ```
    /// use procps_core::helper::{Refusal, Response, VERSION, WriteMessage};
    ///
    /// let mut wire = Vec::new();
    /// wire.write_message(&Response::Refused(Refusal::Malformed))?;
    ///
    /// // The version, the body's length of 2, and a body that contains the
    /// // variant indices of `Refused` and `Malformed`.
    /// assert_eq!(
    ///     wire,
    ///     [&VERSION.to_be_bytes()[..], &[0, 0, 0, 2, 1, 2]].concat()
    /// );
    /// # Ok::<(), procps_core::helper::ProtocolError>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a [`ProtocolError`] if `message` cannot be encoded, if its body
    /// is larger than [`Message::LIMIT`], or if the write fails. The method
    /// writes nothing in the first two cases.
    fn write_message<M: Message>(&mut self, message: &M) -> Result<(), ProtocolError> {
        let mut header = Vec::from(VERSION.to_be_bytes());
        header.resize(HEADER, 0);
        let mut frame = postcard::to_extend(message, header)?;
        let body = frame.len() - HEADER;

        let Some(length) = u32::try_from(body)
            .ok()
            .filter(|length| *length <= M::LIMIT)
        else {
            return Err(ProtocolError::TooLarge {
                length: u64::try_from(body).unwrap_or(u64::MAX),
                limit: M::LIMIT,
            });
        };

        frame[2..HEADER].copy_from_slice(&length.to_be_bytes());
        self.write_all(&frame)?;
        self.flush()?;

        Ok(())
    }
}

impl<W: Write + ?Sized> WriteMessage for W {}
