//! VO lengths for the loader: a line's event resolved in its `Bk_<GUID>.INT` bank
//! (`dishonored_audio::vo`), the length read from the media header.

use crate::load::VoLengths;
use crate::model::VoRef;
use dishonored_audio::vo::VoResolver;

impl VoLengths for VoResolver {
    fn length(&mut self, vo: &VoRef) -> Option<f32> {
        self.resolve(&vo.event, &vo.bank).map(|c| c.length)
    }
}
