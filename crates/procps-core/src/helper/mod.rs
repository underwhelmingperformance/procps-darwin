// SPDX-FileCopyrightText: 2026 Iain Lane <iain@orangesquash.org.uk>
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! The protocol between the tools and `procps-helperd`, the root helper that
//! reads other users' process data.

mod policy;
mod protocol;

pub use policy::Caller;
pub use protocol::{
    Message, ProtocolError, ReadMessage, Refusal, Request, Response, VERSION, WriteMessage,
};
