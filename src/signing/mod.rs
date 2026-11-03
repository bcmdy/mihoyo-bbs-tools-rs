mod ds;

pub use crate::mihoyo::{APP_SALT, BODY_SALT, WEB_SALT};
pub use ds::{
    Clock, DsHeader, DsRandom, DsSigner, FixedClock, FixedRandom, SystemClock, ThreadRandom,
    sign_ds_with, sign_ds2_with,
};
