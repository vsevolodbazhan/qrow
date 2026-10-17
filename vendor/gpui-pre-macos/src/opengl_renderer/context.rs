//! A hardware CGL context. Every caller serializes access and restores the thread's context.
use anyhow::{Result, ensure};
use cgl::*;
use std::{
    ffi::{CString, c_void},
    ptr,
};

// cgl 0.3.2 exports the profile attribute but omits Apple's profile values.
const CORE_3_2_PROFILE: CGLPixelFormatAttribute = 0x3200;

pub(super) struct Context {
    pub raw: CGLContextObj,
    pub format: CGLPixelFormatObj,
}

// Access is serialized by the owner's mutex and CGLLockContext. A context is
// never current on two threads. No GL reference escapes with_current.
unsafe impl Send for Context {}

impl Context {
    pub(super) fn new() -> Result<Self> {
        // CGL selects a Core context through the 3.2 profile token. The
        // driver must expose OpenGL 3.3 or later for instanced drawing.
        let attributes = [
            kCGLPFAOpenGLProfile,
            CORE_3_2_PROFILE,
            kCGLPFAAccelerated,
            kCGLPFANoRecovery,
            kCGLPFAColorSize,
            24,
            kCGLPFAAlphaSize,
            8,
            0,
        ];
        let mut format = ptr::null_mut();
        let mut count = 0;
        let error = unsafe { CGLChoosePixelFormat(attributes.as_ptr(), &mut format, &mut count) };
        ensure!(
            error == 0 && !format.is_null(),
            "hardware OpenGL pixel format unavailable ({error})"
        );
        let mut raw = ptr::null_mut();
        let error = unsafe { CGLCreateContext(format, ptr::null_mut(), &mut raw) };
        if error != 0 || raw.is_null() {
            unsafe { CGLReleasePixelFormat(format) };
            anyhow::bail!("OpenGL context creation failed ({error})");
        }
        let context = Self { raw, format };
        let mut accelerated = 0;
        let error =
            unsafe { CGLDescribePixelFormat(format, 0, kCGLPFAAccelerated, &mut accelerated) };
        ensure!(
            error == 0 && accelerated != 0,
            "OpenGL context is not hardware accelerated"
        );
        Ok(context)
    }
    pub(super) fn with_current<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        Self::with_raw_current(self.raw, work)
    }
    pub(super) fn with_raw_current<T>(
        raw: CGLContextObj,
        work: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let error = unsafe { CGLLockContext(raw) };
        ensure!(error == 0, "cannot lock OpenGL context ({error})");
        let previous = unsafe { CGLGetCurrentContext() };
        struct Guard {
            context: CGLContextObj,
            previous: CGLContextObj,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                unsafe {
                    CGLSetCurrentContext(self.previous);
                    CGLUnlockContext(self.context);
                }
            }
        }
        let _guard = Guard {
            context: raw,
            previous,
        };
        let error = unsafe { CGLSetCurrentContext(raw) };
        ensure!(error == 0, "cannot bind OpenGL context ({error})");
        work()
    }
    pub(super) fn load(&self) -> Result<glow::Context> {
        self.with_current(|| {
            Ok(unsafe {
                glow::Context::from_loader_function(|name| {
                    let name = CString::new(name).expect("GL symbol");
                    libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr())
                        .cast::<c_void>()
                        .cast_const()
                })
            })
        })
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            CGLReleaseContext(self.raw);
            CGLReleasePixelFormat(self.format);
        }
    }
}

pub(super) fn info(gl: &glow::Context) -> Result<String> {
    use glow::HasContext;
    let (version, renderer, attributes, varyings, texture) = unsafe {
        (
            gl.get_parameter_string(glow::VERSION),
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_i32(glow::MAX_VERTEX_ATTRIBS),
            gl.get_parameter_i32(glow::MAX_VERTEX_OUTPUT_COMPONENTS)
                .min(gl.get_parameter_i32(glow::MAX_FRAGMENT_INPUT_COMPONENTS)),
            gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE),
        )
    };
    validate_limits(attributes, varyings, texture)?;
    let v = gl.version();
    ensure!(
        (v.major, v.minor) >= (3, 3),
        "Core instancing requires OpenGL 3.3 or later: {version}"
    );
    let mask = unsafe { gl.get_parameter_i32(glow::CONTEXT_PROFILE_MASK) };
    ensure!(
        mask & glow::CONTEXT_CORE_PROFILE_BIT as i32 != 0,
        "CGL did not create a Core context"
    );
    Ok(format!(
        "{renderer}; {version}; core profile; instanced; hardware accelerated; attributes={attributes}, varying={varyings}, texture={texture}"
    ))
}

fn validate_limits(attributes: i32, varyings: i32, texture: i32) -> Result<()> {
    ensure!(
        attributes >= 14 && varyings >= 48 && texture >= super::atlas::PAGE_SIZE,
        "OpenGL limits are too small: attributes={attributes}, varying={varyings}, texture={texture}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insufficient_hardware_limits_fail_before_scene_drawing() {
        assert!(validate_limits(14, 48, super::super::atlas::PAGE_SIZE).is_ok());
        for (attributes, varyings, texture) in [(13, 48, 2048), (14, 47, 2048), (14, 48, 1024)] {
            assert!(validate_limits(attributes, varyings, texture).is_err());
        }
    }
}

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CGLRetainContext(context: CGLContextObj) -> CGLContextObj;
    pub(super) fn CGLReleaseContext(context: CGLContextObj);
}
