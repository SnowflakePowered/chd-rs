pub const CD_TRACK_PADDING: u32 = 4;
pub const CD_MAX_TRACKS: u32 = 99;    /* AFAIK the theoretical limit */
pub const CD_MAX_SECTOR_DATA: u32 = 2352;
pub const CD_MAX_SUBCODE_DATA: u32 = 96;
pub const CD_FRAME_SIZE: u32 = CD_MAX_SECTOR_DATA + CD_MAX_SUBCODE_DATA;
pub const CD_FRAMES_PER_HUNK: u32 = 8;
pub const CD_METADATA_WORDS: u32 = 1 + (CD_MAX_TRACKS * 6);
