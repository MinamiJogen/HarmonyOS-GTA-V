//! XComponent NativeWindow ownership and the wgpu Vulkan presentation handoff.
//! The UI owns XComponent. A separate native reference keeps its window alive
//! while the GPU thread and wgpu surface still use it.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) mod surface_probe;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle,
    OhosNdkWindowHandle, WindowHandle,
};

#[link(name = "native_window")]
unsafe extern "C" {
    fn OH_NativeWindow_NativeObjectReference(object: *mut c_void) -> i32;
    fn OH_NativeWindow_NativeObjectUnreference(object: *mut c_void) -> i32;
    fn OH_NativeWindow_NativeWindowHandleOpt(window: *mut c_void, code: i32, ...) -> i32;
    fn OH_NativeWindow_NativeWindowSetScalingModeV2(window: *mut c_void, mode: i32) -> i32;
}

#[derive(Debug)]
pub struct NativeWindow {
    handle: NonNull<c_void>,
    width: AtomicU32,
    height: AtomicU32,
}

// NativeWindow is a reference-counted cross-thread platform object. All GPU
// operations run on the dedicated GPU thread; size notifications are atomic.
unsafe impl Send for NativeWindow {}
unsafe impl Sync for NativeWindow {}

impl NativeWindow {
    unsafe fn retain(handle: *mut c_void, width: u32, height: u32) -> anyhow::Result<Self> {
        let handle = NonNull::new(handle).ok_or_else(|| anyhow::anyhow!("null OHNativeWindow"))?;
        let result = unsafe { OH_NativeWindow_NativeObjectReference(handle.as_ptr()) };
        anyhow::ensure!(result == 0, "OHNativeWindow reference failed: {result}");
        Ok(Self { handle, width: AtomicU32::new(width.max(1)), height: AtomicU32::new(height.max(1)) })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width.load(Ordering::Relaxed), self.height.load(Ordering::Relaxed))
    }

    /// Match the native buffer geometry to the guest backbuffer before the
    /// swapchain is configured. XComponent scales that buffer to its UI bounds.
    pub fn set_buffer_geometry(&self, width: u32, height: u32) -> anyhow::Result<()> {
        anyhow::ensure!(width > 0 && height > 0 && width <= i32::MAX as u32 && height <= i32::MAX as u32,
            "invalid native buffer geometry {width}x{height}");
        // SET_BUFFER_GEOMETRY = 0 in native_window/external_window.h API 26.
        let result = unsafe {
            OH_NativeWindow_NativeWindowHandleOpt(self.handle.as_ptr(), 0, width as i32, height as i32)
        };
        anyhow::ensure!(result == 0, "OHNativeWindow SET_BUFFER_GEOMETRY failed: {result}");
        Ok(())
    }

    /// Apply the public window policy after swapchain configuration. Call on
    /// the GPU thread, serialized with its NativeWindow/Vulkan operations.
    /// Success confirms the request only; RS buffer mode needs device evidence.
    pub fn set_aspect_fit(&self) -> anyhow::Result<()> {
        // OHScalingModeV2 in SDK native_window/external_window.h, since API 12.
        const OH_SCALING_MODE_SCALE_FIT_V2: i32 = 4;
        let result = unsafe {
            OH_NativeWindow_NativeWindowSetScalingModeV2(self.handle.as_ptr(), OH_SCALING_MODE_SCALE_FIT_V2)
        };
        eprintln!("[native-window] GPU post-configure aspect-fit request mode=4 result={result}");
        anyhow::ensure!(result == 0, "OHNativeWindow post-configure SCALE_FIT request failed: {result}");
        Ok(())
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        unsafe { OH_NativeWindow_NativeObjectUnreference(self.handle.as_ptr()); }
    }
}

impl HasWindowHandle for NativeWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = OhosNdkWindowHandle::new(self.handle).into();
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

impl HasDisplayHandle for NativeWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::ohos())
    }
}

static WINDOW: Mutex<Option<Arc<NativeWindow>>> = Mutex::new(None);

/// Called once for a newly created XComponent, before starting guest threads.
///
/// # Safety
/// The pointer must be the live OHNativeWindow received in the XComponent
/// surface-created callback. The callback must not destroy it concurrently.
pub unsafe fn prepare_surface(native_window: *mut c_void, width: u32, height: u32) -> anyhow::Result<()> {
    anyhow::ensure!(!crate::gpu::STARTED.load(Ordering::SeqCst), "GPU already started; recreate the ability to replace its surface");
    let window = Arc::new(unsafe { NativeWindow::retain(native_window, width, height)? });
    // High-frequency crash checkpoints are explicitly opt-in for normal play.
    if let Some(root) = std::env::var_os("MUGUET_SUPPORT") {
        let root = std::path::PathBuf::from(root);
        crate::config::configure_game_diagnostic_paths(&root);
    }
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(desc);
    let surface = instance.create_surface(window.clone())?;
    *crate::gpu::SURFACE.lock().unwrap() = Some(crate::gpu::SurfaceHandoff {
        instance, surface, window: window.clone(),
    });
    *WINDOW.lock().unwrap() = Some(window);
    suspend(false);
    eprintln!("[ohos] Vulkan XComponent surface created: {width}x{height}");
    Ok(())
}

pub fn resize(width: u32, height: u32) {
    if let Some(window) = WINDOW.lock().unwrap().as_ref() {
        window.width.store(width.max(1), Ordering::Relaxed);
        window.height.store(height.max(1), Ordering::Relaxed);
    }
}

pub fn suspend(suspended: bool) {
    crate::gpu::SUSPENDED.store(suspended, Ordering::Relaxed);
    crate::gpu::OCCLUDED.store(suspended, Ordering::Relaxed);
    crate::input::set_focus(!suspended);
    crate::audio::suspend(suspended);
}
