//! msvcrt / ucrt: floating point math. Windows passes and returns floats in xmm registers, which
//! `extern "win64"` handles; the maths themselves are the host's libm.

mod c {
    use std::ffi::c_int;
    extern "C" {
    pub fn erf(x: f64) -> f64;
    pub fn erfc(x: f64) -> f64;
    pub fn tgamma(x: f64) -> f64;
    pub fn lgamma(x: f64) -> f64;
    pub fn nextafter(x: f64, y: f64) -> f64;
    pub fn frexp(x: f64, e: *mut c_int) -> f64;
    pub fn ldexp(x: f64, e: c_int) -> f64;
    pub fn modf(x: f64, ip: *mut f64) -> f64;
    pub fn fma(x: f64, y: f64, z: f64) -> f64;
    pub fn remainder(x: f64, y: f64) -> f64;
    pub fn scalbn(x: f64, n: c_int) -> f64;
    pub fn logb(x: f64) -> f64;
    pub fn ilogb(x: f64) -> c_int;
    }
}

fn fpclass(x: f64) -> i32 {
    use std::num::FpCategory::*;
    let neg = x.is_sign_negative();
    match (x.classify(), neg) {
        (Nan, _) => 0x0002,
        (Infinite, true) => 0x0004,
        (Infinite, false) => 0x0200,
        (Normal, true) => 0x0008,
        (Normal, false) => 0x0100,
        (Subnormal, true) => 0x0010,
        (Subnormal, false) => 0x0080,
        (Zero, true) => 0x0020,
        (Zero, false) => 0x0040,
    }
}

crate::shims! { "crt";
    fn sin(x: f64) -> f64 { x.sin() }
    fn cos(x: f64) -> f64 { x.cos() }
    fn tan(x: f64) -> f64 { x.tan() }
    fn asin(x: f64) -> f64 { x.asin() }
    fn acos(x: f64) -> f64 { x.acos() }
    fn atan(x: f64) -> f64 { x.atan() }
    fn atan2(y: f64, x: f64) -> f64 { y.atan2(x) }
    fn sinh(x: f64) -> f64 { x.sinh() }
    fn cosh(x: f64) -> f64 { x.cosh() }
    fn tanh(x: f64) -> f64 { x.tanh() }
    fn asinh(x: f64) -> f64 { x.asinh() }
    fn acosh(x: f64) -> f64 { x.acosh() }
    fn atanh(x: f64) -> f64 { x.atanh() }
    fn exp(x: f64) -> f64 { x.exp() }
    fn exp2(x: f64) -> f64 { x.exp2() }
    fn expm1(x: f64) -> f64 { x.exp_m1() }
    fn log(x: f64) -> f64 { x.ln() }
    fn log10(x: f64) -> f64 { x.log10() }
    fn log2(x: f64) -> f64 { x.log2() }
    fn log1p(x: f64) -> f64 { x.ln_1p() }
    fn logb(x: f64) -> f64 { c::logb(x) }
    fn pow(x: f64, y: f64) -> f64 { x.powf(y) }
    fn sqrt(x: f64) -> f64 { x.sqrt() }
    fn cbrt(x: f64) -> f64 { x.cbrt() }
    fn hypot(x: f64, y: f64) -> f64 { x.hypot(y) }
    fn _hypot(x: f64, y: f64) -> f64 { x.hypot(y) }
    fn ceil(x: f64) -> f64 { x.ceil() }
    fn floor(x: f64) -> f64 { x.floor() }
    fn trunc(x: f64) -> f64 { x.trunc() }
    fn round(x: f64) -> f64 { x.round() }
    fn rint(x: f64) -> f64 { x.round_ties_even() }
    fn nearbyint(x: f64) -> f64 { x.round_ties_even() }
    fn lround(x: f64) -> i32 { x.round() as i32 }
    fn llround(x: f64) -> i64 { x.round() as i64 }
    fn lrint(x: f64) -> i32 { x.round_ties_even() as i32 }
    fn llrint(x: f64) -> i64 { x.round_ties_even() as i64 }
    fn fabs(x: f64) -> f64 { x.abs() }
    fn fmod(x: f64, y: f64) -> f64 { x % y }
    fn remainder(x: f64, y: f64) -> f64 { c::remainder(x, y) }
    fn copysign(x: f64, y: f64) -> f64 { x.copysign(y) }
    fn _copysign(x: f64, y: f64) -> f64 { x.copysign(y) }
    fn _chgsign(x: f64) -> f64 { -x }
    fn fmax(x: f64, y: f64) -> f64 { x.max(y) }
    fn fmin(x: f64, y: f64) -> f64 { x.min(y) }
    fn fdim(x: f64, y: f64) -> f64 { if x > y { x - y } else { 0.0 } }
    fn fma(x: f64, y: f64, z: f64) -> f64 { x.mul_add(y, z) }
    fn ldexp(x: f64, e: i32) -> f64 { c::ldexp(x, e) }
    fn scalbn(x: f64, e: i32) -> f64 { c::scalbn(x, e) }
    fn _scalb(x: f64, e: i32) -> f64 { c::ldexp(x, e) }
    fn ilogb(x: f64) -> i32 { c::ilogb(x) }
    fn frexp(x: f64, e: *mut i32) -> f64 { c::frexp(x, e) }
    fn modf(x: f64, ip: *mut f64) -> f64 { c::modf(x, ip) }
    fn nextafter(x: f64, y: f64) -> f64 { c::nextafter(x, y) }
    fn _nextafter(x: f64, y: f64) -> f64 { c::nextafter(x, y) }
    fn erf(x: f64) -> f64 { c::erf(x) }
    fn erfc(x: f64) -> f64 { c::erfc(x) }
    fn tgamma(x: f64) -> f64 { c::tgamma(x) }
    fn lgamma(x: f64) -> f64 { c::lgamma(x) }
    fn _finite(x: f64) -> i32 { x.is_finite() as i32 }
    fn _isnan(x: f64) -> i32 { x.is_nan() as i32 }
    fn _fpclass(x: f64) -> i32 { fpclass(x) }
    fn sinf(x: f32) -> f32 { x.sin() }
    fn cosf(x: f32) -> f32 { x.cos() }
    fn tanf(x: f32) -> f32 { x.tan() }
    fn asinf(x: f32) -> f32 { x.asin() }
    fn acosf(x: f32) -> f32 { x.acos() }
    fn atanf(x: f32) -> f32 { x.atan() }
    fn atan2f(y: f32, x: f32) -> f32 { y.atan2(x) }
    fn sinhf(x: f32) -> f32 { x.sinh() }
    fn coshf(x: f32) -> f32 { x.cosh() }
    fn tanhf(x: f32) -> f32 { x.tanh() }
    fn expf(x: f32) -> f32 { x.exp() }
    fn logf(x: f32) -> f32 { x.ln() }
    fn log10f(x: f32) -> f32 { x.log10() }
    fn log2f(x: f32) -> f32 { x.log2() }
    fn powf(x: f32, y: f32) -> f32 { x.powf(y) }
    fn sqrtf(x: f32) -> f32 { x.sqrt() }
    fn cbrtf(x: f32) -> f32 { x.cbrt() }
    fn ceilf(x: f32) -> f32 { x.ceil() }
    fn floorf(x: f32) -> f32 { x.floor() }
    fn truncf(x: f32) -> f32 { x.trunc() }
    fn roundf(x: f32) -> f32 { x.round() }
    fn fabsf(x: f32) -> f32 { x.abs() }
    fn fmodf(x: f32, y: f32) -> f32 { x % y }
    fn hypotf(x: f32, y: f32) -> f32 { x.hypot(y) }
    fn copysignf(x: f32, y: f32) -> f32 { x.copysign(y) }
    fn fmaxf(x: f32, y: f32) -> f32 { x.max(y) }
    fn fminf(x: f32, y: f32) -> f32 { x.min(y) }
    fn lroundf(x: f32) -> i32 { x.round() as i32 }
    fn llroundf(x: f32) -> i64 { x.round() as i64 }
}

