use std::{
    ffi::{c_int, c_void},
    num::NonZeroU32,
    ptr::NonNull,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::ffi::{
    MpvClient, MpvError, MpvRenderContext, MpvRenderParam, RENDER_PARAM_ADVANCED_CONTROL,
    RENDER_PARAM_API_TYPE, RENDER_PARAM_INVALID, RENDER_PARAM_SKIP_RENDERING,
    RENDER_PARAM_SW_FORMAT, RENDER_PARAM_SW_POINTER, RENDER_PARAM_SW_SIZE, RENDER_PARAM_SW_STRIDE,
    RENDER_UPDATE_FRAME,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SoftwareRenderConfig {
    pub max_width: NonZeroU32,
    pub max_height: NonZeroU32,
    pub resize_debounce: Duration,
}

impl Default for SoftwareRenderConfig {
    fn default() -> Self {
        Self {
            max_width: NonZeroU32::new(1280).unwrap_or(NonZeroU32::MIN),
            max_height: NonZeroU32::new(720).unwrap_or(NonZeroU32::MIN),
            resize_debounce: Duration::from_millis(100),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SoftwareFrame {
    pub width: NonZeroU32,
    pub height: NonZeroU32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone)]
pub struct SoftwareFrameSource {
    shared: Arc<SoftwareShared>,
}

impl SoftwareFrameSource {
    pub fn set_target_size(&self, width: u32, height: u32) {
        self.shared.control.update_target(width, height);
    }

    pub fn set_visible(&self, visible: bool) {
        self.shared.control.update_visibility(visible);
    }

    pub fn set_wakeup_callback(&self, callback: impl FnMut() + Send + 'static) {
        *lock(&self.shared.callback) = Some(Box::new(callback));
        let should_notify = {
            let mut frames = lock(&self.shared.frames);
            if frames.latest.is_some() && !frames.notification_pending {
                frames.notification_pending = true;
                true
            } else {
                false
            }
        };
        if should_notify {
            self.shared.notify_frame_ready();
        }
    }

    pub fn clear_wakeup_callback(&self) {
        *lock(&self.shared.callback) = None;
    }

    /// Takes the newest frame and drops any older frame that was overwritten.
    pub fn take_latest(&self) -> Option<SoftwareFrame> {
        let mut frames = lock(&self.shared.frames);
        let frame = frames.latest.take();
        frames.notification_pending = false;
        frame
    }
}

struct SoftwareShared {
    control: Arc<WorkerControl>,
    frames: Mutex<FrameSlot>,
    callback: Mutex<Option<Box<dyn FnMut() + Send>>>,
}

impl SoftwareShared {
    fn publish(&self, frame: SoftwareFrame) {
        let should_notify = {
            let mut frames = lock(&self.frames);
            frames.latest = Some(frame);
            if frames.notification_pending {
                false
            } else {
                frames.notification_pending = true;
                true
            }
        };
        if should_notify {
            self.notify_frame_ready();
        }
    }

    fn notify_frame_ready(&self) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some(callback) = lock(&self.callback).as_mut() {
                callback();
            }
        }));
        if result.is_err() {
            lock(&self.frames).notification_pending = false;
        }
    }
}

#[derive(Default)]
struct FrameSlot {
    latest: Option<SoftwareFrame>,
    notification_pending: bool,
}

pub(crate) struct SoftwareRenderRuntime {
    source: SoftwareFrameSource,
    worker: Option<JoinHandle<Result<(), MpvError>>>,
}

impl SoftwareRenderRuntime {
    pub(crate) fn start(
        client: Arc<MpvClient>,
        config: SoftwareRenderConfig,
    ) -> Result<Self, MpvError> {
        let control = Arc::new(WorkerControl::default());
        let shared = Arc::new(SoftwareShared {
            control: control.clone(),
            frames: Mutex::new(FrameSlot::default()),
            callback: Mutex::new(None),
        });
        let source = SoftwareFrameSource {
            shared: shared.clone(),
        };
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("mpv-software-render".to_owned())
            .spawn(move || software_worker(client, config, control, shared, ready_sender))
            .map_err(MpvError::SoftwareThread)?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                source,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(MpvError::SoftwareWorkerPanicked)
            }
        }
    }

    pub(crate) fn source(&self) -> SoftwareFrameSource {
        self.source.clone()
    }

    pub(crate) fn shutdown(mut self) -> Result<(), MpvError> {
        self.source.clear_wakeup_callback();
        self.source.shared.control.shutdown();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| MpvError::SoftwareWorkerPanicked)??;
        }
        Ok(())
    }
}

impl Drop for SoftwareRenderRuntime {
    fn drop(&mut self) {
        self.source.clear_wakeup_callback();
        self.source.shared.control.shutdown();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy)]
struct TargetState {
    width: u32,
    height: u32,
    visible: bool,
    changed_at: Instant,
    revision: u64,
}

impl Default for TargetState {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            visible: false,
            changed_at: Instant::now(),
            revision: 0,
        }
    }
}

#[derive(Default)]
struct WorkerSignals {
    shutdown: bool,
    render_pending: bool,
    target: TargetState,
}

#[derive(Default)]
struct WorkerControl {
    signals: Mutex<WorkerSignals>,
    condvar: Condvar,
}

impl WorkerControl {
    fn request_render(&self) {
        lock(&self.signals).render_pending = true;
        self.condvar.notify_one();
    }

    fn update_target(&self, width: u32, height: u32) {
        let mut signals = lock(&self.signals);
        if (signals.target.width, signals.target.height) != (width, height) {
            signals.target.width = width;
            signals.target.height = height;
            signals.target.changed_at = Instant::now();
            signals.target.revision = signals.target.revision.wrapping_add(1);
            self.condvar.notify_one();
        }
    }

    fn update_visibility(&self, visible: bool) {
        let mut signals = lock(&self.signals);
        if signals.target.visible != visible {
            signals.target.visible = visible;
            signals.target.revision = signals.target.revision.wrapping_add(1);
            self.condvar.notify_one();
        }
    }

    fn shutdown(&self) {
        lock(&self.signals).shutdown = true;
        self.condvar.notify_one();
    }
}

fn software_worker(
    client: Arc<MpvClient>,
    config: SoftwareRenderConfig,
    control: Arc<WorkerControl>,
    shared: Arc<SoftwareShared>,
    ready: mpsc::SyncSender<Result<(), MpvError>>,
) -> Result<(), MpvError> {
    let mut context = match SoftwareContext::create(client, control.clone()) {
        Ok(context) => {
            let _ = ready.send(Ok(()));
            context
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let mut active_size = None;
    let mut applied_revision = 0;
    let mut buffer = AlignedBuffer::default();

    loop {
        let (shutdown, render_pending, target, target_settled) = {
            let mut signals = lock(&control.signals);
            while !signals.shutdown
                && !signals.render_pending
                && signals.target.revision == applied_revision
            {
                signals = control
                    .condvar
                    .wait(signals)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if signals.shutdown {
                (true, false, signals.target, false)
            } else {
                let elapsed = signals.target.changed_at.elapsed();
                let settled = elapsed >= config.resize_debounce;
                if signals.target.revision != applied_revision
                    && !settled
                    && signals.target.width > 0
                    && signals.target.height > 0
                {
                    let remaining = config.resize_debounce.saturating_sub(elapsed);
                    let (next, _) = control
                        .condvar
                        .wait_timeout(signals, remaining)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    signals = next;
                }
                let settled = signals.target.changed_at.elapsed() >= config.resize_debounce;
                let render_pending = std::mem::take(&mut signals.render_pending);
                (false, render_pending, signals.target, settled)
            }
        };
        if shutdown {
            break;
        }

        let update_flags = context.update();
        let frame_pending = render_pending && update_flags & RENDER_UPDATE_FRAME != 0;
        let size_changed = target_settled && target.revision != applied_revision;
        if size_changed {
            applied_revision = target.revision;
            active_size = capped_dimensions(target.width, target.height, config);
        }

        if !target.visible || active_size.is_none() {
            if frame_pending {
                context.skip_rendering()?;
            }
            continue;
        }
        if !frame_pending && !size_changed {
            continue;
        }

        let Some((width, height)) = active_size else {
            continue;
        };
        let frame = context.render(width, height, &mut buffer)?;
        shared.publish(frame);
    }
    Ok(())
}

struct SoftwareContext {
    raw: NonNull<MpvRenderContext>,
    client: Arc<MpvClient>,
    control: Arc<WorkerControl>,
}

impl SoftwareContext {
    fn create(client: Arc<MpvClient>, control: Arc<WorkerControl>) -> Result<Self, MpvError> {
        let mut advanced_control: c_int = 1;
        let mut params = [
            MpvRenderParam {
                param_type: RENDER_PARAM_API_TYPE,
                data: c"sw".as_ptr().cast_mut().cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_ADVANCED_CONTROL,
                data: (&mut advanced_control as *mut c_int).cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let mut raw = std::ptr::null_mut();
        // SAFETY: Parameter storage remains valid for this synchronous call and
        // the initialized client outlives the returned render context.
        client.api.result(unsafe {
            (client.api.render_create)(&mut raw, client.handle(), params.as_mut_ptr())
        })?;
        let raw = NonNull::new(raw).ok_or(MpvError::NullRenderContext)?;
        // SAFETY: control is kept alive by this context until the callback is
        // explicitly unregistered in Drop.
        unsafe {
            (client.api.render_set_update_callback)(
                raw.as_ptr(),
                Some(software_render_update),
                Arc::as_ptr(&control).cast_mut().cast(),
            )
        };
        Ok(Self {
            raw,
            client,
            control,
        })
    }

    fn update(&mut self) -> u64 {
        // SAFETY: raw is owned by this worker thread and valid until Drop.
        unsafe { (self.client.api.render_update)(self.raw.as_ptr()) }
    }

    fn skip_rendering(&mut self) -> Result<(), MpvError> {
        let mut skip: c_int = 1;
        let mut params = [
            MpvRenderParam {
                param_type: RENDER_PARAM_SKIP_RENDERING,
                data: (&mut skip as *mut c_int).cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        // SAFETY: The context is worker-owned and parameter pointers remain
        // valid for the synchronous render call.
        self.client
            .api
            .result(unsafe { (self.client.api.render)(self.raw.as_ptr(), params.as_mut_ptr()) })
    }

    fn render(
        &mut self,
        width: NonZeroU32,
        height: NonZeroU32,
        buffer: &mut AlignedBuffer,
    ) -> Result<SoftwareFrame, MpvError> {
        let width_usize = width.get() as usize;
        let height_usize = height.get() as usize;
        let row_bytes = width_usize
            .checked_mul(4)
            .ok_or_else(|| MpvError::Software("software frame row overflowed".to_owned()))?;
        let stride = align_up(row_bytes, 64)?;
        let required = stride
            .checked_mul(height_usize)
            .ok_or_else(|| MpvError::Software("software frame allocation overflowed".to_owned()))?;
        buffer.ensure_len(required)?;

        let mut size = [
            i32::try_from(width.get())
                .map_err(|_| MpvError::Software("software frame width is too large".to_owned()))?,
            i32::try_from(height.get())
                .map_err(|_| MpvError::Software("software frame height is too large".to_owned()))?,
        ];
        let mut stride_param = stride;
        let mut params = [
            MpvRenderParam {
                param_type: RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_SW_FORMAT,
                data: c"rgb0".as_ptr().cast_mut().cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_SW_STRIDE,
                data: (&mut stride_param as *mut usize).cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_SW_POINTER,
                data: buffer.as_mut_ptr().cast::<c_void>(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        // SAFETY: The context is worker-owned and the aligned writable buffer
        // covers stride * height bytes for the duration of the call.
        self.client
            .api
            .result(unsafe { (self.client.api.render)(self.raw.as_ptr(), params.as_mut_ptr()) })?;

        let tight_len = row_bytes
            .checked_mul(height_usize)
            .ok_or_else(|| MpvError::Software("tight RGBA frame overflowed".to_owned()))?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(tight_len).map_err(|error| {
            MpvError::Software(format!("could not allocate RGBA frame: {error}"))
        })?;
        rgba.resize(tight_len, 0);
        for row in 0..height_usize {
            let source_start = row * stride;
            let target_start = row * row_bytes;
            rgba[target_start..target_start + row_bytes]
                .copy_from_slice(&buffer.as_slice()[source_start..source_start + row_bytes]);
        }
        for alpha in rgba.iter_mut().skip(3).step_by(4) {
            *alpha = u8::MAX;
        }
        Ok(SoftwareFrame {
            width,
            height,
            rgba: rgba.into(),
        })
    }
}

impl Drop for SoftwareContext {
    fn drop(&mut self) {
        // SAFETY: No other thread uses this context; removing the callback
        // first prevents any later access to WorkerControl.
        unsafe {
            (self.client.api.render_set_update_callback)(
                self.raw.as_ptr(),
                None,
                std::ptr::null_mut(),
            );
            (self.client.api.render_free)(self.raw.as_ptr());
        }
        let _ = &self.control;
    }
}

unsafe extern "C" fn software_render_update(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: SoftwareContext retains the Arc and unregisters this callback
    // before releasing it.
    let control = unsafe { &*context.cast::<WorkerControl>() };
    control.request_render();
}

#[derive(Default)]
struct AlignedBuffer {
    storage: Vec<u8>,
    offset: usize,
    len: usize,
}

impl AlignedBuffer {
    fn ensure_len(&mut self, len: usize) -> Result<(), MpvError> {
        let required = len
            .checked_add(63)
            .ok_or_else(|| MpvError::Software("aligned buffer size overflowed".to_owned()))?;
        if self.storage.len() < required {
            self.storage.clear();
            self.storage.try_reserve_exact(required).map_err(|error| {
                MpvError::Software(format!("could not allocate aligned frame: {error}"))
            })?;
            self.storage.resize(required, 0);
        }
        let address = self.storage.as_ptr() as usize;
        self.offset = (64 - address % 64) % 64;
        self.len = len;
        Ok(())
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        // SAFETY: offset is within the 63-byte over-allocation and len was
        // validated by ensure_len.
        unsafe { self.storage.as_mut_ptr().add(self.offset) }
    }

    fn as_slice(&self) -> &[u8] {
        &self.storage[self.offset..self.offset + self.len]
    }
}

fn align_up(value: usize, alignment: usize) -> Result<usize, MpvError> {
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
        .ok_or_else(|| MpvError::Software("stride alignment overflowed".to_owned()))
}

fn capped_dimensions(
    width: u32,
    height: u32,
    config: SoftwareRenderConfig,
) -> Option<(NonZeroU32, NonZeroU32)> {
    let width = NonZeroU32::new(width)?;
    let height = NonZeroU32::new(height)?;
    let max_width = config.max_width.get();
    let max_height = config.max_height.get();
    if width.get() <= max_width && height.get() <= max_height {
        return Some((width, height));
    }

    let width_limited = u64::from(width.get()) * u64::from(max_height)
        > u64::from(height.get()) * u64::from(max_width);
    let (scaled_width, scaled_height) = if width_limited {
        (
            max_width,
            ((u64::from(height.get()) * u64::from(max_width)) / u64::from(width.get())).max(1)
                as u32,
        )
    } else {
        (
            ((u64::from(width.get()) * u64::from(max_height)) / u64::from(height.get())).max(1)
                as u32,
            max_height,
        )
    };
    Some((
        NonZeroU32::new(scaled_width)?,
        NonZeroU32::new(scaled_height)?,
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{AlignedBuffer, SoftwareRenderConfig, align_up, capped_dimensions};

    #[test]
    fn stride_and_buffer_are_64_byte_aligned() {
        assert_eq!(align_up(513, 64).expect("valid stride"), 576);
        let mut buffer = AlignedBuffer::default();
        buffer.ensure_len(4096).expect("valid allocation");
        assert_eq!(buffer.as_mut_ptr() as usize % 64, 0);
    }

    #[test]
    fn render_cap_preserves_wide_and_tall_aspect_ratios() {
        let config = SoftwareRenderConfig::default();
        let wide = capped_dimensions(3840, 2160, config).expect("wide dimensions");
        assert_eq!((wide.0.get(), wide.1.get()), (1280, 720));
        let tall = capped_dimensions(1080, 1920, config).expect("tall dimensions");
        assert_eq!((tall.0.get(), tall.1.get()), (405, 720));
    }
}
