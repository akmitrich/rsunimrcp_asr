#![allow(clippy::missing_safety_doc)]
mod recognizer;
mod speech_detector;

use std::io::Write;
use std::mem::size_of;

use recognizer::RecogBuffer;
use rsunimrcp_engine::RawEngine;
use rsunimrcp_sys::uni;
use rsunimrcp_sys::*;
use speech_detector::SpeechDetectorEvent;

const RECOG_ENGINE_TASK_NAME: &[u8; 16] = b"Rust ASR-Engine\0";

pub static ENGINE_VTABLE: uni::mrcp_engine_method_vtable_t = uni::mrcp_engine_method_vtable_t {
    destroy: Some(engine_destroy),
    open: Some(engine_open),
    close: Some(engine_close),
    create_channel: Some(engine_create_channel),
};

static CHANNEL_VTABLE: uni::mrcp_engine_channel_method_vtable_t =
    uni::mrcp_engine_channel_method_vtable_t {
        destroy: Some(channel_destroy),
        open: Some(channel_open),
        close: Some(channel_close),
        process_request: Some(channel_process_request),
    };

static STREAM_VTABLE: uni::mpf_audio_stream_vtable_t = uni::mpf_audio_stream_vtable_t {
    destroy: Some(stream_destroy),
    open_rx: None,
    close_rx: None,
    read_frame: None,
    open_tx: Some(stream_open),
    close_tx: Some(stream_close),
    write_frame: Some(stream_write),
    trace: None,
};

#[repr(C)]
struct MrcpRecogEngine {
    task: *mut uni::apt_consumer_task_t,
    raw_engine: *mut RawEngine,
}

#[derive(Debug)]
#[repr(C)]
struct MrcpRecogChannel {
    custom_engine: *mut MrcpRecogEngine,
    channel: *mut uni::mrcp_engine_channel_t,
    recog_request: *mut uni::mrcp_message_t,
    stop_response: *mut uni::mrcp_message_t,
    audio_buffer: *mut RecogBuffer,
}

#[repr(C)]
enum RecogMsgType {
    OpenChannel,
    CloseChannel,
    RequestProcess,
}

#[repr(C)]
struct RecogMsg {
    type_: RecogMsgType,
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
}

#[no_mangle]
pub static mut mrcp_plugin_version: uni::mrcp_plugin_version_t = uni::mrcp_plugin_version_t {
    major: uni::PLUGIN_MAJOR_VERSION as i32,
    minor: uni::PLUGIN_MINOR_VERSION as i32,
    patch: uni::PLUGIN_PATCH_VERSION as i32,
    is_dev: 0,
};

#[no_mangle]
pub unsafe extern "C" fn mrcp_plugin_create(pool: *mut uni::apr_pool_t) -> *mut uni::mrcp_engine_t {
    env_logger::init();
    log::trace!("Going to create Rust ASR-Engine on pool = {:?}", pool);

    let custom_engine = uni::apr_palloc(pool, size_of::<MrcpRecogEngine>()) as *mut MrcpRecogEngine;
    (*custom_engine).raw_engine = std::ptr::null_mut() as _;
    let msg_pool = uni::apt_task_msg_pool_create_dynamic(size_of::<RecogMsg>(), pool);
    (*custom_engine).task = uni::apt_consumer_task_create(custom_engine as _, msg_pool, pool);
    if (*custom_engine).task.is_null() {
        return std::ptr::null_mut();
    }
    let task = uni::apt_consumer_task_base_get((*custom_engine).task);
    uni::apt_task_name_set(task, RECOG_ENGINE_TASK_NAME.as_ptr() as _);
    let vtable = uni::apt_task_vtable_get(task);
    if !vtable.is_null() {
        (*vtable).process_msg = Some(rs_recog_msg_process);
    }
    let engine = uni::mrcp_engine_create(
        uni::MRCP_RECOGNIZER_RESOURCE as _,
        custom_engine as _,
        &ENGINE_VTABLE as _,
        pool,
    );
    log::info!("ASR-Engine created: {:?}", engine);
    engine
}

unsafe extern "C" fn engine_destroy(engine: *mut uni::mrcp_engine_t) -> uni::apt_bool_t {
    let custom_engine = (*engine).obj as *mut MrcpRecogEngine;
    log::trace!(
        "Destroy Engine {:?}. Custom engine = {:?}",
        engine,
        custom_engine
    );
    if !(*custom_engine).task.is_null() {
        let task = uni::apt_consumer_task_base_get((*custom_engine).task);
        let destroyed = uni::apt_task_destroy(task);
        (*custom_engine).task = std::ptr::null_mut() as _;
        log::trace!("Task {:?} destroyed = {:?}", task, destroyed);
    }
    RawEngine::destroy((*custom_engine).raw_engine);
    uni::TRUE
}

unsafe extern "C" fn engine_open(engine: *mut uni::mrcp_engine_t) -> uni::apt_bool_t {
    let custom_engine = (*engine).obj as *mut MrcpRecogEngine;
    log::trace!(
        "Open Engine {:?}. Custom engine = {:?}",
        engine,
        custom_engine
    );
    if !(*custom_engine).task.is_null() {
        let task = uni::apt_consumer_task_base_get((*custom_engine).task);
        let started = uni::apt_task_start(task);
        log::debug!("Task = {:?} started = {:?}.", task, started);
    }
    (*custom_engine).raw_engine = RawEngine::leaked(engine);
    log::info!("Opened with raw Engine: {:?}", (*custom_engine).raw_engine);
    inline_mrcp_engine_open_respond(engine, uni::TRUE)
}

unsafe extern "C" fn engine_close(engine: *mut uni::mrcp_engine_t) -> uni::apt_bool_t {
    let custom_engine = (*engine).obj as *mut MrcpRecogEngine;
    log::info!(
        "Close Engine {:?}. Custom engine = {:?}",
        engine,
        custom_engine
    );
    if !(*custom_engine).task.is_null() {
        let task = uni::apt_consumer_task_base_get((*custom_engine).task);
        let terminated = uni::apt_task_terminate(task, uni::TRUE);
        log::trace!("Task = {:?} terminated = {:?}.", task, terminated);
    }
    inline_mrcp_engine_close_respond(engine)
}

unsafe extern "C" fn engine_create_channel(
    engine: *mut uni::mrcp_engine_t,
    pool: *mut uni::apr_pool_t,
) -> *mut uni::mrcp_engine_channel_t {
    log::debug!("Engine {:?} is going to create a channel", engine);
    let custom_engine = (*engine).obj as *mut MrcpRecogEngine;
    let rs_engine = (*(*custom_engine).raw_engine).engine();

    let custom_channel =
        uni::apr_palloc(pool, size_of::<MrcpRecogChannel>()) as *mut MrcpRecogChannel;
    (*custom_channel).custom_engine = (*engine).obj as _;
    (*custom_channel).recog_request = std::ptr::null_mut() as _;
    (*custom_channel).stop_response = std::ptr::null_mut() as _;
    (*custom_channel).audio_buffer = RecogBuffer::leaked(rs_engine);

    let capabilities = inline_mpf_sink_stream_capabilities_create(pool);
    inline_mpf_codec_capabilities_add(
        &mut (*capabilities).codecs as _,
        uni::MPF_SAMPLE_RATE_8000 as _,
        b"LPCM\0".as_ptr() as _,
    );

    let termination = uni::mrcp_engine_audio_termination_create(
        custom_channel as _,
        &STREAM_VTABLE as _,
        capabilities,
        pool,
    );
    (*custom_channel).channel = uni::mrcp_engine_channel_create(
        engine,
        &CHANNEL_VTABLE as _,
        custom_channel as _,
        termination,
        pool,
    );
    log::info!(
        "Engine created channel = {:?} ({:6})",
        (*custom_channel).channel,
        (*(*custom_engine).raw_engine).channel_opened()
    );
    (*custom_channel).channel
}

pub unsafe extern "C" fn channel_destroy(
    channel: *mut uni::mrcp_engine_channel_t,
) -> uni::apt_bool_t {
    log::debug!("Channel {:?} destroy.", channel);
    let custom_channel = (*channel).method_obj as *mut MrcpRecogChannel;
    RecogBuffer::destroy((*custom_channel).audio_buffer);
    uni::TRUE
}

pub unsafe extern "C" fn channel_open(channel: *mut uni::mrcp_engine_channel_t) -> uni::apt_bool_t {
    log::debug!("Channel {:?} open.", channel);
    if !(*channel).attribs.is_null() {
        let header = uni::apr_table_elts((*channel).attribs);
        let entry = (*header).elts as *mut uni::apr_table_entry_t;
        for i in 0..(*header).nelts {
            let entry = entry.offset(i as _);
            let key = std::ffi::CStr::from_ptr((*entry).key);
            let val = std::ffi::CStr::from_ptr((*entry).val);
            log::info!("Attrib name {:?} value {:?}", key, val);
        }
    }
    rs_recog_msg_signal(
        RecogMsgType::OpenChannel,
        channel,
        std::ptr::null_mut() as _,
    )
}

unsafe extern "C" fn channel_close(channel: *mut uni::mrcp_engine_channel_t) -> uni::apt_bool_t {
    log::info!("Channel {:?} close.", channel);
    rs_recog_msg_signal(
        RecogMsgType::CloseChannel,
        channel,
        std::ptr::null_mut() as _,
    )
}

unsafe extern "C" fn channel_process_request(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::debug!(
        "Channel {:?} process request {:?}.",
        channel,
        (*request).start_line.method_id
    );
    rs_recog_msg_signal(RecogMsgType::RequestProcess, channel, request)
}

unsafe fn rs_recog_channel_recognize(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let custom_channel = (*channel).method_obj as *mut MrcpRecogChannel;
    let descriptor = uni::mrcp_engine_sink_stream_codec_get(channel);

    if descriptor.is_null() {
        log::error!("Failed to Get Codec Descriptor from channel {:?}", channel);
        (*response).start_line.status_code = uni::MRCP_STATUS_CODE_METHOD_FAILED;
        return uni::FALSE;
    }
    let recog_header = rsunimrcp_sys::headers::RecogHeaders::new(request);
    log::info!(
        "Channel {:?}\nRecognize-headers: {:?}",
        channel,
        recog_header
    );
    (*(*custom_channel).audio_buffer).prepare(recog_header);

    (*response).start_line.request_state = uni::MRCP_REQUEST_STATE_INPROGRESS;
    inline_mrcp_engine_channel_message_send(channel, response);

    (*custom_channel).recog_request = request;
    uni::TRUE
}

unsafe fn rs_recog_channel_stop(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    log::info!("Process Stop request {:?} for {:?}", request, channel);
    let custom_channel = (*channel).method_obj as *mut MrcpRecogChannel;
    (*custom_channel).stop_response = response;
    uni::TRUE
}

unsafe fn rs_recog_channel_timers_start(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
    response: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let custom_channel = (*channel).method_obj as *mut MrcpRecogChannel;
    (*(*custom_channel).audio_buffer).start_input_timers();
    log::info!("Send TIMERS START {:?} for {:?}", request, channel);
    inline_mrcp_engine_channel_message_send(channel, response)
}

unsafe fn rs_recog_channel_request_dispatch(
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let mut processed = uni::FALSE;
    let response = uni::mrcp_response_create(request, (*request).pool);
    let method_id = (*request).start_line.method_id;
    match method_id as u32 {
        uni::RECOGNIZER_SET_PARAMS => {}
        uni::RECOGNIZER_GET_PARAMS => {}
        uni::RECOGNIZER_DEFINE_GRAMMAR => {}
        uni::RECOGNIZER_RECOGNIZE => {
            processed = rs_recog_channel_recognize(channel, request, response);
        }
        uni::RECOGNIZER_GET_RESULT => {}
        uni::RECOGNIZER_START_INPUT_TIMERS => {
            processed = rs_recog_channel_timers_start(channel, request, response);
        }
        uni::RECOGNIZER_STOP => {
            processed = rs_recog_channel_stop(channel, request, response);
        }
        x => {
            log::error!("Unexpected method id={}", x);
        }
    }
    if processed == uni::FALSE {
        log::warn!("Method {:?} not processed", method_id);
        inline_mrcp_engine_channel_message_send(channel, response);
    }
    uni::TRUE
}

pub unsafe extern "C" fn stream_destroy(_stream: *mut uni::mpf_audio_stream_t) -> uni::apt_bool_t {
    uni::TRUE
}

pub unsafe extern "C" fn stream_open(
    _stream: *mut uni::mpf_audio_stream_t,
    _codec: *mut uni::mpf_codec_t,
) -> uni::apt_bool_t {
    uni::TRUE
}

pub unsafe extern "C" fn stream_close(_stream: *mut uni::mpf_audio_stream_t) -> uni::apt_bool_t {
    uni::TRUE
}

unsafe fn rs_recog_start_of_input(recog_channel: *mut MrcpRecogChannel) -> uni::apt_bool_t {
    let message = uni::mrcp_event_create(
        (*recog_channel).recog_request,
        uni::RECOGNIZER_START_OF_INPUT as _,
        (*(*recog_channel).recog_request).pool,
    );
    if message.is_null() {
        log::error!("Unable to create event START OF INPUT");
        return uni::FALSE;
    }
    log::info!(
        "Send START OF INPUT message in {:?}",
        (*recog_channel).channel
    );
    (*message).start_line.request_state = uni::MRCP_REQUEST_STATE_INPROGRESS;
    inline_mrcp_engine_channel_message_send((*recog_channel).channel, message)
}

unsafe fn rs_recog_result_load(
    recognized: &str,
    message: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let generic_header = inline_mrcp_generic_header_prepare(message);
    if !generic_header.is_null() {
        inline_apt_string_assign(
            &mut (*generic_header).content_type as _,
            b"text/plain; charset=UTF-8\0".as_ptr() as _,
            (*message).pool,
        );
        uni::mrcp_generic_header_property_add(message, uni::GENERIC_HEADER_CONTENT_TYPE as _);
    }
    let result = recognized.as_bytes();
    inline_apt_string_assign_n(
        &mut (*message).body as _,
        result.as_ptr() as _,
        result.len(),
        (*message).pool,
    );

    uni::TRUE
}

unsafe fn rs_recog_recognition_process(
    recog_channel: *mut MrcpRecogChannel,
    recog_event: SpeechDetectorEvent,
) -> uni::apt_bool_t {
    let mut recognized = String::new();
    let cause = match recog_event {
        SpeechDetectorEvent::None => return uni::FALSE,
        SpeechDetectorEvent::Activity => {
            log::info!("Detected Voice Activity in {:?}", (*recog_channel).channel);
            if !(*(*recog_channel).audio_buffer).input_started() {
                (*(*recog_channel).audio_buffer).start_input();
                return rs_recog_start_of_input(recog_channel);
            } else {
                return uni::TRUE;
            }
        }
        SpeechDetectorEvent::Inactivity { duration } => {
            log::info!(
                "Detected Voice {:?} in {:?}",
                recog_event,
                (*recog_channel).channel
            );
            (*(*recog_channel).audio_buffer).recognize(duration);
            return uni::TRUE;
        }
        SpeechDetectorEvent::DurationTimeout => {
            log::info!(
                "Detected Duration Timeout in {:?}",
                (*recog_channel).channel
            );
            (*(*recog_channel).audio_buffer)
                .recognize((*(*recog_channel).audio_buffer).duration_timeout());
            return uni::TRUE;
        }
        SpeechDetectorEvent::Noinput => {
            log::info!("Detected Noinput. Channel {:?}", (*recog_channel).channel);
            uni::RECOGNIZER_COMPLETION_CAUSE_NO_INPUT_TIMEOUT
        }
        SpeechDetectorEvent::Recognizing => match (*(*recog_channel).audio_buffer).load_result() {
            None => return uni::FALSE,
            Some(result) => {
                recognized = result;
                uni::RECOGNIZER_COMPLETION_CAUSE_SUCCESS
            }
        },
    };
    let message = uni::mrcp_event_create(
        (*recog_channel).recog_request,
        uni::RECOGNIZER_RECOGNITION_COMPLETE as _,
        (*(*recog_channel).recog_request).pool,
    );
    if message.is_null() {
        log::error!("Unable to create event RECOGNITION COMPLETE");
        return uni::FALSE;
    }
    let recog_header =
        inline_mrcp_resource_header_prepare(message) as *mut uni::mrcp_recog_header_t;
    if !recog_header.is_null() {
        (*recog_header).completion_cause = cause;
        uni::mrcp_resource_header_property_add(
            message,
            uni::RECOGNIZER_HEADER_COMPLETION_CAUSE as _,
        );
    }
    (*message).start_line.request_state = uni::MRCP_REQUEST_STATE_COMPLETE;
    if cause == uni::RECOGNIZER_COMPLETION_CAUSE_SUCCESS {
        if recognized.is_empty() {
            (*(*recog_channel).audio_buffer).restart_writing();
            return uni::FALSE;
        }
        rs_recog_result_load(recognized.as_str(), message);
        log::info!(
            "Load for {:?}: {:?} ({} bytes)",
            (*recog_channel).channel,
            recognized,
            recognized.as_bytes().len()
        );
    }
    (*recog_channel).recog_request = std::ptr::null_mut() as _;
    inline_mrcp_engine_channel_message_send((*recog_channel).channel, message)
}

pub unsafe extern "C" fn stream_write(
    stream: *mut uni::mpf_audio_stream_t,
    frame: *const uni::mpf_frame_t,
) -> uni::apt_bool_t {
    let custom_channel = (*stream).obj as *mut MrcpRecogChannel;
    if !(*custom_channel).stop_response.is_null() {
        inline_mrcp_engine_channel_message_send(
            (*custom_channel).channel,
            (*custom_channel).stop_response,
        );
        (*custom_channel).stop_response = std::ptr::null_mut() as _;
        (*custom_channel).recog_request = std::ptr::null_mut() as _;
        return uni::TRUE;
    }
    if !(*custom_channel).recog_request.is_null() {
        if ((*frame).type_ & (uni::MEDIA_FRAME_TYPE_EVENT as i32))
            == uni::MEDIA_FRAME_TYPE_EVENT as i32
        {
            if (*frame).marker == uni::MPF_MARKER_START_OF_EVENT as i32 {
                log::info!(
                    "Detected Start of Event id: {}",
                    (*frame).event_frame.event_id()
                );
            } else if (*frame).marker == uni::MPF_MARKER_END_OF_EVENT as i32 {
                log::info!(
                    "Detected End of Event id: {}, duration: {}",
                    (*frame).event_frame.event_id(),
                    (*frame).event_frame.duration()
                )
            }
        } else {
            let buf = std::slice::from_raw_parts(
                (*frame).codec_frame.buffer as *mut u8,
                (*frame).codec_frame.size,
            );
            (*(*custom_channel).audio_buffer).write(buf).ok();
            let event = (*(*custom_channel).audio_buffer).detector_event();
            rs_recog_recognition_process(custom_channel, event);
        }
    }
    uni::TRUE
}

unsafe extern "C" fn rs_recog_msg_signal(
    type_: RecogMsgType,
    channel: *mut uni::mrcp_engine_channel_t,
    request: *mut uni::mrcp_message_t,
) -> uni::apt_bool_t {
    let mut status = uni::FALSE;
    let custom_channel = (*channel).method_obj as *mut MrcpRecogChannel;
    let custom_engine = (*custom_channel).custom_engine;
    let task = uni::apt_consumer_task_base_get((*custom_engine).task);
    let msg = uni::apt_task_msg_get(task);
    if !msg.is_null() {
        (*msg).type_ = uni::TASK_MSG_USER as _;
        let recog_msg = (*msg).data.as_mut_ptr() as *mut RecogMsg;
        (*recog_msg).type_ = type_;
        (*recog_msg).channel = channel;
        (*recog_msg).request = request;
        status = uni::apt_task_msg_signal(task, msg);
    }
    status
}

unsafe extern "C" fn rs_recog_msg_process(
    _task: *mut uni::apt_task_t,
    msg: *mut uni::apt_task_msg_t,
) -> uni::apt_bool_t {
    let recog_msg = (*msg).data.as_mut_ptr() as *mut RecogMsg;
    match (*recog_msg).type_ {
        RecogMsgType::OpenChannel => {
            inline_mrcp_engine_channel_open_respond((*recog_msg).channel, uni::TRUE);
        }
        RecogMsgType::CloseChannel => {
            inline_mrcp_engine_channel_close_respond((*recog_msg).channel);
        }
        RecogMsgType::RequestProcess => {
            rs_recog_channel_request_dispatch((*recog_msg).channel, (*recog_msg).request);
        }
    }
    uni::TRUE
}
