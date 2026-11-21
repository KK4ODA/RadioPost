// SPDX-License-Identifier: MIT OR Apache-2.0
//! Choosing a gateway (`docs/design/04-gateway-selection.md`): where the station is, and what the
//! client keeps of every call, so that later choices can learn from it.
//!
//! * [`attempts`]: one record of each call placed, kept in the first home and never rewritten.
//! * [`classify`]: why a call ended as it did, from what the modem and the session said.
//! * [`directory`]: the gateway list as the operator sees it: favourites, blocks, gateways of their
//!   own, ages, and what the call records say of each.
//! * [`hf`]: HF channels predicted both ways by VOACAP and read through the modem's profile.
//! * [`hillshade`]: the terrain map's background, drawn from the tiles held.
//! * [`position`]: the places, the GPS and the position in use.
//! * [`terrain`]: VHF and UHF gateways judged over the terrain, from Copernicus tiles fetched when
//!   the operator asks.

pub mod attempts;
pub mod classify;
pub mod directory;
pub mod hf;
pub mod hillshade;
pub mod position;
pub mod terrain;
