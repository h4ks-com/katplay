//! The console's video: libchiaki's ffmpeg decoder, and the conversion of
//! what it decodes to the pixels katplay shows.

use std::ffi::c_void;
use std::mem::zeroed;
use std::ptr;
use std::sync::Arc;

use super::session::Codec;
use super::{Error, Log, Result, c_string, check, sys};

/// One decoded picture as RGBA pixels of the size `pull` asked for, and how many pictures the
/// console sent before it that were lost on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picture {
    pub rgba: Vec<u8>,
    pub lost: i32,
}

/// `FFmpeg`'s own value for its bilinear scaler, which bindgen leaves out.
const SWS_BILINEAR: i32 = 2;

pub(crate) struct VideoDecoder {
    decoder: Box<sys::ChiakiFfmpegDecoder>,
    // We hold these so the pointers libchiaki keeps stay valid.
    _log: Arc<Log>,
    _signal: Box<Box<dyn Fn() + Send>>,
    /// The converter for the last source and target it saw.
    converter: *mut sys::SwsContext,
    converted_from: (i32, i32, i32),
    converted_to: (u32, u32),
    software: *mut sys::AVFrame,
}

/// An `FFmpeg` frame until it is dropped.
struct Frame(*mut sys::AVFrame);

impl Drop for Frame {
    fn drop(&mut self) {
        // SAFETY: the pointer is the frame `pull_frame` handed out, freed exactly here.
        unsafe { sys::av_frame_free(ptr::from_mut(&mut self.0)) };
    }
}

impl VideoDecoder {
    /// Decodes `codec` at up to `max_fps`, on the hardware decoder `hardware`
    /// names when there is one, and calls `on_picture` from the session's
    /// thread whenever a picture waits.
    pub(crate) fn new(
        log: Arc<Log>,
        codec: Codec,
        max_fps: u32,
        hardware: Option<&str>,
        on_picture: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let mut signal: Box<Box<dyn Fn() + Send>> = Box::new(Box::new(on_picture));
        let hardware = hardware
            .map(|name| c_string(name, "decoder name"))
            .transpose()?;
        // SAFETY: chiaki_ffmpeg_decoder_init expects an all-zero decoder and fills it in.
        let mut decoder = Box::new(unsafe { zeroed::<sys::ChiakiFfmpegDecoder>() });
        let (capture, kept) = Log::capture();
        // SAFETY: `decoder` is zeroed as required, the strings are live, and the callback matches.
        let started = check(unsafe {
            sys::chiaki_ffmpeg_decoder_init(
                ptr::from_mut(&mut *decoder),
                capture.raw(),
                codec.raw(),
                max_fps,
                hardware.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                ptr::null_mut(),
                Some(picture_ready),
                ptr::from_mut(&mut *signal).cast(),
            )
        });
        if started.is_err() {
            let log = kept.lock().map(|log| log.clone()).unwrap_or_default();
            return Err(Error::Startup {
                what: "video decoder",
                log,
            });
        }
        decoder.log = log.raw();
        // SAFETY: the decoder is initialized, and the frame is a new empty one we own.
        let software = unsafe { sys::av_frame_alloc() };
        if software.is_null() {
            return Err(Error::Video("out of memory for a video frame".to_owned()));
        }
        Ok(Self {
            decoder,
            _log: log,
            _signal: signal,
            converter: ptr::null_mut(),
            converted_from: (0, 0, 0),
            converted_to: (0, 0),
            software,
        })
    }

    pub(super) fn raw(&self) -> *mut sys::ChiakiFfmpegDecoder {
        ptr::from_ref(&*self.decoder).cast_mut()
    }

    /// The newest picture since the last call, scaled to `size`; `None` when
    /// none is waiting. Pictures that arrived in between are skipped, so the
    /// one shown is always the newest the console sent.
    pub(crate) fn pull(&mut self, size: (u32, u32)) -> Result<Option<Picture>> {
        let mut lost: i32 = 0;
        // SAFETY: the decoder was initialized in `new`, and only the calling thread pulls from it.
        let pulled = unsafe {
            sys::chiaki_ffmpeg_decoder_pull_frame(ptr::from_mut(&mut *self.decoder), &raw mut lost)
        };
        if pulled.frame.is_null() {
            return Ok(None);
        }
        let mut frame = Frame(pulled.frame);
        let rgba = self.convert(&mut frame, size)?;
        Ok(Some(Picture { rgba, lost }))
    }

    fn convert(&mut self, frame: &mut Frame, size: (u32, u32)) -> Result<Vec<u8>> {
        let mut source = frame.0;
        // SAFETY: `frame` is live for this call, and the scratch frame is ours; sws_scale reads
        // the source and writes exactly `size` worth of pixels into the buffer we pass.
        unsafe {
            if !(*source).hw_frames_ctx.is_null() {
                sys::av_frame_unref(self.software);
                if sys::av_hwframe_transfer_data(self.software, source, 0) < 0 {
                    return Err(Error::Video(
                        "cannot read a picture from the hardware decoder".to_owned(),
                    ));
                }
                source = self.software;
            }
            let (width, height, format) = ((*source).width, (*source).height, (*source).format);
            self.fit_converter(width, height, format, size)?;
            let mut rgba = vec![0u8; (size.0 * size.1 * 4) as usize];
            let source_rows: [*const u8; 8] = (*source).data.map(<*mut u8>::cast_const);
            let rows = [rgba.as_mut_ptr()];
            let strides = [(size.0 * 4).cast_signed()];
            sys::sws_scale(
                self.converter,
                source_rows.as_ptr(),
                (*source).linesize.as_ptr(),
                0,
                height,
                rows.as_ptr(),
                strides.as_ptr(),
            );
            Ok(rgba)
        }
    }

    fn fit_converter(
        &mut self,
        width: i32,
        height: i32,
        format: i32,
        size: (u32, u32),
    ) -> Result<()> {
        if !self.converter.is_null()
            && self.converted_from == (width, height, format)
            && self.converted_to == size
        {
            return Ok(());
        }
        // SAFETY: sws_getContext returns a converter we own, and sws_freeContext releases the one made before.
        unsafe {
            if !self.converter.is_null() {
                sys::sws_freeContext(self.converter);
            }
            self.converter = sys::sws_getContext(
                width,
                height,
                format,
                size.0.cast_signed(),
                size.1.cast_signed(),
                sys::AV_PIX_FMT_RGBA,
                SWS_BILINEAR,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
        }
        if self.converter.is_null() {
            return Err(Error::Video("cannot scale the picture".to_owned()));
        }
        self.converted_from = (width, height, format);
        self.converted_to = size;
        Ok(())
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        // SAFETY: the converter and the scratch frame are the ones made in this decoder.
        unsafe {
            if !self.converter.is_null() {
                sys::sws_freeContext(self.converter);
            }
            sys::av_frame_free(ptr::from_mut(&mut self.software));
            sys::chiaki_ffmpeg_decoder_fini(ptr::from_mut(&mut *self.decoder));
        }
    }
}

unsafe extern "C" fn picture_ready(_decoder: *mut sys::ChiakiFfmpegDecoder, user: *mut c_void) {
    // SAFETY: `user` is the closure boxed in `new`, which lives until the decoder is dropped.
    let signal = unsafe { &*user.cast::<Box<dyn Fn() + Send>>() };
    signal();
}
