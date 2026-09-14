#!/bin/bash
sed -i 's/pub wb_high_auto: bool,/pub wb_high_mode: WhiteBalanceMode,/g' src/ui/review.rs
sed -i 's/wb_high_auto: true,/wb_high_mode: WhiteBalanceMode::Neutral,/g' src/ui/review.rs
sed -i '/pub struct NegadoctorModes/i \
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]\
pub enum WhiteBalanceMode {\
    #[default]\
    Neutral,\
    SampledAuto(SampleRect),\
    Manual,\
}\
' src/ui/review.rs
