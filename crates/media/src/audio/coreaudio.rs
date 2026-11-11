//! macOS output through AudioToolbox's AudioQueue: three buffers cycle
//! between the queue's callback thread and the mixer thread.

use super::Backend;
use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};

type OSStatus = i32;
type AudioQueueRef = *mut c_void;

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioQueueBuffer {
    audio_data_bytes_capacity: u32,
    audio_data: *mut c_void,
    audio_data_byte_size: u32,
    user_data: *mut c_void,
    packet_description_capacity: u32,
    packet_descriptions: *mut c_void,
    packet_description_count: u32,
}

type AudioQueueBufferRef = *mut AudioQueueBuffer;
type OutputCallback = extern "C" fn(*mut c_void, AudioQueueRef, AudioQueueBufferRef);

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioQueueNewOutput(format: *const AudioStreamBasicDescription, callback: OutputCallback, user: *mut c_void, run_loop: *const c_void, mode: *const c_void, flags: u32, out: *mut AudioQueueRef) -> OSStatus;
    fn AudioQueueAllocateBuffer(queue: AudioQueueRef, bytes: u32, out: *mut AudioQueueBufferRef) -> OSStatus;
    fn AudioQueueEnqueueBuffer(queue: AudioQueueRef, buf: AudioQueueBufferRef, n: u32, descs: *const c_void) -> OSStatus;
    fn AudioQueueStart(queue: AudioQueueRef, at: *const c_void) -> OSStatus;
    fn AudioQueueDispose(queue: AudioQueueRef, immediate: u8) -> OSStatus;
}

const K_AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
const K_AUDIO_FORMAT_FLAG_IS_PACKED: u32 = 8;
const RATE: u32 = 48_000;
const FRAMES_PER_BUFFER: usize = 960;
const BUFFERS: usize = 3;

/// Raw buffer pointer moved between threads.
struct BufPtr(AudioQueueBufferRef);
unsafe impl Send for BufPtr {}

extern "C" fn on_buffer_done(user: *mut c_void, _q: AudioQueueRef, buf: AudioQueueBufferRef) {
    // SAFETY: `user` is the boxed Sender owned by the backend, alive until
    // the queue is disposed.
    let tx = unsafe { &*(user as *const Sender<BufPtr>) };
    let _ = tx.send(BufPtr(buf));
}

pub(crate) struct CoreAudioBackend {
    queue: AudioQueueRef,
    free: Receiver<BufPtr>,
    _tx: Box<Sender<BufPtr>>,
    current: Option<AudioQueueBufferRef>,
}

unsafe impl Send for CoreAudioBackend {}

impl CoreAudioBackend {
    pub(crate) fn open() -> Result<CoreAudioBackend, String> {
        let (tx, rx) = channel();
        let tx = Box::new(tx);
        let fmt = AudioStreamBasicDescription {
            sample_rate: RATE as f64,
            format_id: K_AUDIO_FORMAT_LINEAR_PCM,
            format_flags: K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_PACKED,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };
        let mut queue: AudioQueueRef = std::ptr::null_mut();
        // SAFETY: documented AudioQueue usage; the sender outlives the queue.
        unsafe {
            let st = AudioQueueNewOutput(&fmt, on_buffer_done, &*tx as *const Sender<BufPtr> as *mut c_void, std::ptr::null(), std::ptr::null(), 0, &mut queue);
            if st != 0 || queue.is_null() {
                return Err(format!("AudioQueueNewOutput failed ({st})"));
            }
            for _ in 0..BUFFERS {
                let mut b: AudioQueueBufferRef = std::ptr::null_mut();
                if AudioQueueAllocateBuffer(queue, (FRAMES_PER_BUFFER * 8) as u32, &mut b) == 0 && !b.is_null() {
                    // Prime with silence.
                    std::ptr::write_bytes((*b).audio_data as *mut u8, 0, FRAMES_PER_BUFFER * 8);
                    (*b).audio_data_byte_size = (FRAMES_PER_BUFFER * 8) as u32;
                    AudioQueueEnqueueBuffer(queue, b, 0, std::ptr::null());
                }
            }
            let st = AudioQueueStart(queue, std::ptr::null());
            if st != 0 {
                AudioQueueDispose(queue, 1);
                return Err(format!("AudioQueueStart failed ({st})"));
            }
        }
        Ok(CoreAudioBackend { queue, free: rx, _tx: tx, current: None })
    }
}

impl Backend for CoreAudioBackend {
    fn rate(&self) -> u32 {
        RATE
    }
    fn channels(&self) -> usize {
        2
    }
    fn wait(&mut self) -> Result<usize, String> {
        if self.current.is_none() {
            let b = self.free.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| "audio queue stalled".to_string())?;
            self.current = Some(b.0);
        }
        Ok(FRAMES_PER_BUFFER)
    }
    fn write(&mut self, data: &[f32]) -> Result<(), String> {
        let Some(b) = self.current.take() else { return Ok(()) };
        let n = data.len().min(FRAMES_PER_BUFFER * 2);
        // SAFETY: the buffer was handed back by the queue and holds
        // FRAMES_PER_BUFFER stereo float frames.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), (*b).audio_data as *mut f32, n);
            (*b).audio_data_byte_size = (n * 4) as u32;
            let st = AudioQueueEnqueueBuffer(self.queue, b, 0, std::ptr::null());
            if st != 0 {
                return Err(format!("AudioQueueEnqueueBuffer failed ({st})"));
            }
        }
        Ok(())
    }
    fn latency(&self) -> f64 {
        (FRAMES_PER_BUFFER * (BUFFERS - 1)) as f64 / RATE as f64
    }
    fn name(&self) -> &'static str {
        "coreaudio"
    }
}

impl Drop for CoreAudioBackend {
    fn drop(&mut self) {
        // SAFETY: dispose once; callbacks stop before the sender drops.
        unsafe { AudioQueueDispose(self.queue, 1) };
    }
}
