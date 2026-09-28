//! `ICameraDeviceSession`: stream configuration, request templates, and
//! the capture loop.
//!
//! Requests queue in order. One worker thread takes each, waits for its
//! buffers' acquire fences and asks the host module for the next frame
//! written into all of the request's buffers. A second thread sends the
//! shutters and results, in order, so the worker is waiting for the next
//! frame while cameraserver takes the last one (each result is a binder
//! call that returns only once cameraserver has queued the buffers to
//! their consumers). Metadata travels in the parcelables, not in fast message
//! queues: both queue getters return an empty descriptor, which
//! cameraserver takes as "no queue".

use std::collections::{HashMap, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use android_hardware_camera_common::aidl::android::hardware::camera::common::Status::Status;
use android_hardware_camera_device::aidl::android::hardware::camera::device::{
    BufferCache::BufferCache, BufferStatus::BufferStatus, CameraMetadata::CameraMetadata,
    CameraOfflineSessionInfo::CameraOfflineSessionInfo, CaptureRequest::CaptureRequest,
    CaptureResult::CaptureResult, ErrorCode::ErrorCode, ErrorMsg::ErrorMsg, HalStream::HalStream,
    ICameraDeviceCallback::ICameraDeviceCallback, ICameraDeviceSession::ICameraDeviceSession,
    ICameraOfflineSession::ICameraOfflineSession, NotifyMsg::NotifyMsg,
    RequestTemplate::RequestTemplate, ShutterMsg::ShutterMsg, StreamBuffer::StreamBuffer,
    StreamConfiguration::StreamConfiguration, StreamConfigurationMode::StreamConfigurationMode,
    StreamRotation::StreamRotation, StreamType::StreamType,
};
use android_hardware_camera_metadata::aidl::android::hardware::camera::metadata::CameraMetadataTag::CameraMetadataTag as Tag;
use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_common_fmq::aidl::android::hardware::common::fmq::{
    MQDescriptor::MQDescriptor, SynchronizedReadWrite::SynchronizedReadWrite,
};
use android_hardware_graphics_common::aidl::android::hardware::graphics::common::{
    BufferUsage::BufferUsage, PixelFormat::PixelFormat,
};
use binder::{Interface, ParcelFileDescriptor, Strong};
use darwin_hostcall::camera::{self, Output};
use darwin_hostcall::{errno, guest};

use crate::buffer::Buffer;
use crate::characteristics::{self, Camera, FPS, PIPELINE_DEPTH, format};
use crate::metadata::Metadata;

pub fn status(s: Status) -> binder::Status {
    binder::Status::new_service_specific_error(s.0, None)
}

/// Why a stream configuration is not supported, if it is not.
pub fn check_config(camera: &Camera, c: &StreamConfiguration) -> Result<(), String> {
    if c.operationMode != StreamConfigurationMode::NORMAL_MODE {
        return Err(format!("operation mode {:?}", c.operationMode));
    }
    if c.streams.is_empty() {
        return Err("no streams".into());
    }
    let (mut stalling, mut processed) = (0, 0);
    for s in &c.streams {
        if s.streamType != StreamType::OUTPUT || s.rotation != StreamRotation::ROTATION_0 {
            return Err(format!("stream {}: input or rotated", s.id));
        }
        if s.width <= 0
            || s.height <= 0
            || !camera.supports(s.format.0, s.width as u32, s.height as u32)
        {
            return Err(format!(
                "stream {}: format {:#x} {}x{}",
                s.id, s.format.0, s.width, s.height
            ));
        }
        if s.format.0 == format::BLOB {
            stalling += 1;
        } else {
            processed += 1;
        }
    }
    if stalling > 1 || processed > 2 {
        return Err(format!("{processed} processed and {stalling} JPEG streams"));
    }
    Ok(())
}

struct Out {
    stream: i32,
    buffer: i64,
    fence: Option<OwnedFd>,
}

struct Request {
    frame: i32,
    settings: Metadata,
    outputs: Vec<Out>,
}

#[derive(Clone, Copy)]
struct StreamSize {
    width: u32,
    height: u32,
}

#[derive(Default)]
struct State {
    streams: HashMap<i32, StreamSize>,
    buffers: HashMap<(i32, i64), Arc<Buffer>>,
    queue: VecDeque<Request>,
    /// The latest request's settings, for requests that repeat them.
    settings: Metadata,
    /// The host session of the current configuration.
    host: Option<u64>,
    /// The sequence number of the last frame delivered.
    seq: u64,
    /// The worker is capturing a request.
    busy: bool,
    /// `flush` is draining the queue; the worker takes no new request.
    flushing: bool,
    closed: bool,
}

/// Messages and a result for the delivery thread.
type Delivery = (Vec<NotifyMsg>, CaptureResult);

/// Deliveries queued and not yet sent.
#[derive(Default)]
struct Outstanding {
    count: Mutex<usize>,
    sent: Condvar,
}

struct Inner {
    device: u32,
    camera: Camera,
    state: Mutex<State>,
    changed: Condvar,
    deliveries: Sender<Delivery>,
    outstanding: Arc<Outstanding>,
}

pub struct Session {
    inner: Arc<Inner>,
    worker: Mutex<Option<JoinHandle<()>>>,
    in_use: Arc<AtomicBool>,
}

impl Interface for Session {}

impl Session {
    pub fn new(
        device: u32,
        camera: Camera,
        callback: Strong<dyn ICameraDeviceCallback>,
        in_use: Arc<AtomicBool>,
    ) -> Session {
        let (deliveries, queue) = channel();
        let outstanding = Arc::new(Outstanding::default());
        {
            let outstanding = outstanding.clone();
            std::thread::Builder::new()
                .name("camera-results".into())
                .spawn(move || deliver(callback, queue, outstanding))
                .expect("spawn the result thread");
        }
        let inner = Arc::new(Inner {
            device,
            camera,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            deliveries,
            outstanding,
        });
        let worker = {
            let inner = inner.clone();
            std::thread::Builder::new()
                .name("camera-capture".into())
                .spawn(move || inner.run())
                .expect("spawn the capture thread")
        };
        Session {
            inner,
            worker: Mutex::new(Some(worker)),
            in_use,
        }
    }
}

/// Send each delivery: its messages, then its result. Ends when the
/// session is gone.
fn deliver(
    callback: Strong<dyn ICameraDeviceCallback>,
    queue: Receiver<Delivery>,
    outstanding: Arc<Outstanding>,
) {
    for (msgs, result) in queue {
        if let Err(e) = callback.notify(&msgs) {
            log::error!("notify: {e}");
        }
        if let Err(e) = callback.processCaptureResult(&[result]) {
            log::error!("processCaptureResult: {e}");
        }
        *outstanding.count.lock().unwrap() -= 1;
        outstanding.sent.notify_all();
    }
}

fn fence_handle(fd: Option<OwnedFd>) -> NativeHandle {
    NativeHandle {
        fds: fd.map(ParcelFileDescriptor::new).into_iter().collect(),
        ints: Vec::new(),
    }
}

/// Wait for an acquire fence to signal (a sync fence polls readable).
fn wait_fence(fd: &OwnedFd) {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll on one fd we hold.
    unsafe { libc::poll(&mut p, 1, 1000) };
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn run(&self) {
        loop {
            let mut st = self.lock();
            while (st.queue.is_empty() || st.flushing) && !st.closed {
                st = self.changed.wait(st).unwrap();
            }
            if st.closed {
                return;
            }
            let request = st.queue.pop_front().unwrap();
            st.busy = true;
            self.changed.notify_all();
            let buffers: Vec<_> = request
                .outputs
                .iter()
                .map(|o| {
                    let size = st.streams.get(&o.stream).copied();
                    (size, st.buffers.get(&(o.stream, o.buffer)).cloned())
                })
                .collect();
            let (host, after) = (st.host, st.seq);
            drop(st);

            let seq = self.capture(request, buffers, host, after);

            let mut st = self.lock();
            st.busy = false;
            st.seq = st.seq.max(seq);
            self.changed.notify_all();
        }
    }

    /// Capture one request; returns the frame's sequence number (or
    /// `after` when there was none).
    fn capture(
        &self,
        request: Request,
        buffers: Vec<(Option<StreamSize>, Option<Arc<Buffer>>)>,
        host: Option<u64>,
        after: u64,
    ) -> u64 {
        for o in &request.outputs {
            if let Some(f) = &o.fence {
                wait_fence(f);
            }
        }
        let s = &request.settings;
        let quality = s.u8(Tag::ANDROID_JPEG_QUALITY).unwrap_or(95);
        let rotation = s.i32(Tag::ANDROID_JPEG_ORIENTATION).unwrap_or(0);
        let mut outputs: Vec<Output> = Vec::new();
        let mut slots = Vec::new();
        for (i, (size, buf)) in buffers.iter().enumerate() {
            let o = match (size, buf) {
                (Some(size), Some(b)) => b.output(size.width, size.height, quality, rotation),
                _ => None,
            };
            if let Some(o) = o {
                slots.push(Some(outputs.len()));
                outputs.push(o);
            } else {
                log::error!("frame {}: buffer {} cannot be written", request.frame, i);
                slots.push(None);
            }
        }
        let Some(session) = host else {
            self.fail(request);
            return after;
        };
        let mut frame = camera::Frame {
            session,
            after,
            timeout_ms: if after == 0 { 3000 } else { 1000 },
            output_count: outputs.len() as u32,
            outputs: outputs.as_mut_ptr() as u64,
            ..Default::default()
        };
        if let Err(e) = guest::camera_frame(&mut frame) {
            if e.0 == errno::EAGAIN {
                log::error!(
                    "frame {}: no camera frame within {} ms",
                    request.frame,
                    frame.timeout_ms
                );
            } else {
                log::error!("frame {}: capture failed: {e:?}", request.frame);
            }
            self.fail(request);
            return after;
        }

        let mut msgs = vec![NotifyMsg::Shutter(ShutterMsg {
            frameNumber: request.frame,
            timestamp: frame.timestamp_ns,
            readoutTimestamp: frame.timestamp_ns,
        })];
        let mut returned = Vec::new();
        for (o, (slot, (_, buf))) in request.outputs.iter().zip(slots.iter().zip(&buffers)) {
            let written = slot.map_or(0, |i| outputs[i].written);
            let ok = written > 0;
            if ok
                && let (Some(i), Some(b)) = (slot, buf)
                && outputs[*i].format == camera::format::BLOB
            {
                b.finish_jpeg(written as usize);
            }
            if !ok {
                msgs.push(NotifyMsg::Error(ErrorMsg {
                    frameNumber: request.frame,
                    errorStreamId: o.stream,
                    errorCode: ErrorCode::ERROR_BUFFER,
                }));
            }
            returned.push(StreamBuffer {
                streamId: o.stream,
                bufferId: o.buffer,
                status: if ok {
                    BufferStatus::OK
                } else {
                    BufferStatus::ERROR
                },
                ..Default::default()
            });
        }
        let result = characteristics::result(&request.settings, frame.timestamp_ns);
        self.send(
            msgs,
            CaptureResult {
                frameNumber: request.frame,
                result: CameraMetadata {
                    metadata: result.to_bytes(),
                },
                outputBuffers: returned,
                partialResult: 1,
                ..empty_result()
            },
        );
        frame.seq
    }

    /// Return a request unfilled: `ERROR_REQUEST`, and every buffer with
    /// status `ERROR` and its acquire fence as the release fence.
    fn fail(&self, request: Request) {
        let msg = NotifyMsg::Error(ErrorMsg {
            frameNumber: request.frame,
            errorStreamId: -1,
            errorCode: ErrorCode::ERROR_REQUEST,
        });
        let returned = request
            .outputs
            .into_iter()
            .map(|o| StreamBuffer {
                streamId: o.stream,
                bufferId: o.buffer,
                status: BufferStatus::ERROR,
                releaseFence: fence_handle(o.fence),
                ..Default::default()
            })
            .collect();
        self.send(
            vec![msg],
            CaptureResult {
                frameNumber: request.frame,
                outputBuffers: returned,
                ..empty_result()
            },
        );
    }

    /// Queue messages and a result for the delivery thread.
    fn send(&self, msgs: Vec<NotifyMsg>, result: CaptureResult) {
        *self.outstanding.count.lock().unwrap() += 1;
        if self.deliveries.send((msgs, result)).is_err() {
            *self.outstanding.count.lock().unwrap() -= 1;
        }
    }

    /// Wait until every queued delivery was sent.
    fn wait_sent(&self) {
        let mut n = self.outstanding.count.lock().unwrap();
        while *n > 0 {
            n = self.outstanding.sent.wait(n).unwrap();
        }
    }

    /// Stop taking requests, wait for the one in progress, and take the
    /// queue.
    fn drain(&self, mut st: MutexGuard<'_, State>) -> Vec<Request> {
        st.flushing = true;
        while st.busy {
            st = self.changed.wait(st).unwrap();
        }
        let drained = st.queue.drain(..).collect();
        st.flushing = false;
        self.changed.notify_all();
        drained
    }
}

fn empty_result() -> CaptureResult {
    CaptureResult {
        inputBuffer: StreamBuffer {
            streamId: -1,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn close_host(st: &mut State) {
    if let Some(s) = st.host.take() {
        let _ = guest::camera_close(s);
    }
}

#[allow(non_snake_case)]
impl ICameraDeviceSession for Session {
    fn close(&self) -> binder::Result<()> {
        let mut st = self.inner.lock();
        if st.closed {
            return Ok(());
        }
        st.closed = true;
        self.inner.changed.notify_all();
        let drained = self.inner.drain(st);
        for r in drained {
            self.inner.fail(r);
        }
        if let Some(w) = self.worker.lock().unwrap().take() {
            let _ = w.join();
        }
        self.inner.wait_sent();
        let mut st = self.inner.lock();
        close_host(&mut st);
        st.buffers.clear();
        self.in_use.store(false, Ordering::Release);
        log::info!("camera {}: session closed", self.inner.device);
        Ok(())
    }

    fn configureStreams(&self, config: &StreamConfiguration) -> binder::Result<Vec<HalStream>> {
        let camera = &self.inner.camera;
        if let Err(why) = check_config(camera, config) {
            log::error!("configureStreams: unsupported: {why}");
            return Err(status(Status::ILLEGAL_ARGUMENT));
        }
        let mut st = self.inner.lock();
        if st.closed || st.busy || !st.queue.is_empty() {
            return Err(status(Status::ILLEGAL_ARGUMENT));
        }
        close_host(&mut st);
        st.streams = config
            .streams
            .iter()
            .map(|s| {
                let size = StreamSize {
                    width: s.width as u32,
                    height: s.height as u32,
                };
                (s.id, size)
            })
            .collect();
        let State {
            streams, buffers, ..
        } = &mut *st;
        buffers.retain(|(stream, _), _| streams.contains_key(stream));
        let width = config.streams.iter().map(|s| s.width).max().unwrap_or(0) as u32;
        let height = config.streams.iter().map(|s| s.height).max().unwrap_or(0) as u32;
        let mut open = camera::Open {
            device: self.inner.device,
            width,
            height,
            fps: FPS as u32,
            ..Default::default()
        };
        match guest::camera_open(&mut open) {
            Ok(s) => {
                st.host = Some(s);
                let source = if open.source == camera::source::CAMERA {
                    "the camera"
                } else {
                    "a test pattern (no camera access or the camera is suspended; see the host log)"
                };
                log::info!(
                    "camera {}: {} streams, capturing {width}x{height} from {source}",
                    self.inner.device,
                    config.streams.len()
                );
            }
            Err(e) => {
                log::error!("camera {}: host open failed: {e:?}", self.inner.device);
                return Err(status(if e.0 == errno::EBUSY {
                    Status::CAMERA_IN_USE
                } else {
                    Status::INTERNAL_ERROR
                }));
            }
        }
        // CAMERA_OUTPUT | CPU_WRITE_OFTEN: the host writes with the CPU.
        let producer = BufferUsage((1 << 17) | 0x30);
        Ok(config
            .streams
            .iter()
            .map(|s| HalStream {
                id: s.id,
                overrideFormat: PixelFormat(characteristics::override_format(
                    s.format.0, s.usage.0,
                )),
                producerUsage: producer,
                consumerUsage: BufferUsage(0),
                maxBuffers: PIPELINE_DEPTH as i32,
                overrideDataSpace: s.dataSpace,
                physicalCameraId: String::new(),
                supportOffline: false,
            })
            .collect())
    }

    fn constructDefaultRequestSettings(
        &self,
        t: RequestTemplate,
    ) -> binder::Result<CameraMetadata> {
        if !characteristics::template_supported(t.0) {
            return Err(status(Status::ILLEGAL_ARGUMENT));
        }
        Ok(CameraMetadata {
            metadata: characteristics::template(&self.inner.camera, t.0).to_bytes(),
        })
    }

    fn flush(&self) -> binder::Result<()> {
        let st = self.inner.lock();
        let drained = self.inner.drain(st);
        for r in drained {
            self.inner.fail(r);
        }
        self.inner.wait_sent();
        Ok(())
    }

    fn getCaptureRequestMetadataQueue(
        &self,
    ) -> binder::Result<MQDescriptor<i8, SynchronizedReadWrite>> {
        Ok(MQDescriptor::default())
    }

    fn getCaptureResultMetadataQueue(
        &self,
    ) -> binder::Result<MQDescriptor<i8, SynchronizedReadWrite>> {
        Ok(MQDescriptor::default())
    }

    fn isReconfigurationRequired(
        &self,
        _old: &CameraMetadata,
        _new: &CameraMetadata,
    ) -> binder::Result<bool> {
        // No session parameters.
        Ok(false)
    }

    fn processCaptureRequest(
        &self,
        requests: &[CaptureRequest],
        caches: &[BufferCache],
    ) -> binder::Result<i32> {
        let illegal = || status(Status::ILLEGAL_ARGUMENT);
        let mut st = self.inner.lock();
        if st.closed {
            return Err(illegal());
        }
        for c in caches {
            st.buffers.remove(&(c.streamId, c.bufferId));
        }
        for r in requests {
            while st.queue.len() >= PIPELINE_DEPTH as usize && !st.closed {
                st = self.inner.changed.wait(st).unwrap();
            }
            let settings = if r.settings.metadata.is_empty() {
                if st.settings.is_empty() {
                    log::error!("frame {}: no settings", r.frameNumber);
                    return Err(illegal());
                }
                st.settings.clone()
            } else {
                Metadata::parse(&r.settings.metadata).ok_or_else(illegal)?
            };
            let mut outputs = Vec::new();
            for b in &r.outputBuffers {
                if !st.streams.contains_key(&b.streamId) {
                    return Err(illegal());
                }
                let key = (b.streamId, b.bufferId);
                if !b.buffer.fds.is_empty() {
                    let buf = Buffer::import(&b.buffer).ok_or_else(|| {
                        log::error!("frame {}: cannot map buffer {}", r.frameNumber, b.bufferId);
                        illegal()
                    })?;
                    st.buffers.insert(key, Arc::new(buf));
                } else if !st.buffers.contains_key(&key) {
                    return Err(illegal());
                }
                let fence = b
                    .acquireFence
                    .fds
                    .first()
                    .and_then(|f| f.as_ref().try_clone().ok());
                outputs.push(Out {
                    stream: b.streamId,
                    buffer: b.bufferId,
                    fence,
                });
            }
            if outputs.is_empty() || r.inputBuffer.streamId != -1 {
                return Err(illegal());
            }
            st.settings = settings.clone();
            st.queue.push_back(Request {
                frame: r.frameNumber,
                settings,
                outputs,
            });
            self.inner.changed.notify_all();
        }
        Ok(requests.len() as i32)
    }

    fn signalStreamFlush(&self, _streams: &[i32], _counter: i32) -> binder::Result<()> {
        // Buffers are returned as each request completes; nothing waits.
        Ok(())
    }

    fn switchToOffline(
        &self,
        _streams: &[i32],
        _info: &mut CameraOfflineSessionInfo,
    ) -> binder::Result<Strong<dyn ICameraOfflineSession>> {
        Err(status(Status::OPERATION_NOT_SUPPORTED))
    }

    fn repeatingRequestEnd(&self, _frame: i32, _streams: &[i32]) -> binder::Result<()> {
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = ICameraDeviceSession::close(self);
    }
}
