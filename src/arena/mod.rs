// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena allocators. Small and fast.

mod debug;
mod release;
mod scratch;
mod string;

pub use self::debug::Arena;
pub use self::scratch::{ScratchArena, init, scratch_arena};
pub use self::string::ArenaString;
