//! IMA ADPCM WEMs.

use crate::wem::{Pcm, Riff};
use anyhow::{Result, bail};

pub fn decode_ima(_r: &Riff) -> Result<Pcm> {
    bail!("IMA ADPCM not implemented")
}
