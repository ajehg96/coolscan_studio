pub mod analysis;
pub mod color;
pub mod negadoctor;
pub mod roll;

pub use analysis::{
    analyse_pre_white_balance, finish_after_white_balance, sample_highlight_wb, sample_shadow_wb,
    ImageSampleStats, SampleRect, TechnicalAnalysis, WorkingImage,
};
pub use color::{
    ColorError, ColorTransform, IdentityColorTransform, ScannerColorPipeline, WorkingColorSpace,
    DEFAULT_LS40_ICC_BYTES,
};
pub use negadoctor::{
    auto_dmax, auto_highlight_wb, auto_paper_black, auto_print_exposure, auto_scan_bias,
    auto_shadow_wb, render_positive, NegadoctorError, NegadoctorParams, PreparedNegadoctor,
    THRESHOLD as NEGADOCTOR_THRESHOLD,
};
pub use roll::{PreparedFrame, RollId, RollProfile, RollProfileError, ScannerProfile};



