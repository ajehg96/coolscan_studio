pub mod analysis;
pub mod color;
pub mod negadoctor;
pub mod orientation;
pub mod roll;

pub use analysis::{
    ImageSampleStats, SampleRect, TechnicalAnalysis, WorkingImage, analyse_pre_white_balance,
    finish_after_white_balance, sample_highlight_wb, sample_shadow_wb,
};
pub use color::{
    ColorError, ColorTransform, DEFAULT_LS40_ICC_BYTES, IdentityColorTransform,
    ScannerColorPipeline, WorkingColorSpace,
};
pub use negadoctor::{
    NegadoctorError, NegadoctorParams, PreparedNegadoctor, THRESHOLD as NEGADOCTOR_THRESHOLD,
    auto_dmax, auto_highlight_wb, auto_paper_black, auto_print_exposure, auto_scan_bias,
    auto_shadow_wb, render_positive,
};
pub use orientation::{Orientation, OrientationScope};
pub use roll::{
    FilmStock, PreparedFrame, RollCalibration, RollId, RollProfile, RollProfileError,
    ScannerProfile,
};
