//! 取帧线程：按需渲染一帧，转成 `RenderImage` 推给界面。

use super::*;

pub(super) fn render_loop(
    api: Arc<MpvApi>,
    handle: *mut ffi::MpvHandle,
    shared: Arc<Shared>,
    signal: Arc<RenderSignal>,
) {
    let mut context: *mut MpvRenderContext = std::ptr::null_mut();
    let api_type = ffi::RENDER_API_TYPE_SW.as_ptr() as *mut c_void;
    let mut params = [
        MpvRenderParam {
            kind: render_param::API_TYPE,
            data: api_type,
        },
        MpvRenderParam {
            kind: render_param::INVALID,
            data: std::ptr::null_mut(),
        },
    ];

    let rc = unsafe { (api.render_context_create)(&mut context, handle, params.as_mut_ptr()) };
    if rc < 0 {
        let message = api.error_text(rc);
        shared.notify(PlayerEvent::Error(format!("创建渲染上下文失败: {message}")));
        return;
    }

    unsafe {
        (api.render_context_set_update_callback)(
            context,
            Some(on_render_update),
            Arc::as_ptr(&signal) as *mut c_void,
        );
    }

    // 复用缓冲区：尺寸不变时不重新分配。mpv 文档要求 stride 与指针按 64 字节对齐，
    // 否则内部会走「整帧拷贝」的慢路径，所以这里自己做对齐，而不是直接扔一个 Vec。
    let mut buffer = FrameBuffer::default();
    let mut current = RenderTarget::default();
    // 「区域尺寸变了，但还没按新尺寸送出一帧」。
    //
    // 暂停时 mpv 不会再产生新帧（`RENDER_UPDATE_FRAME` 始终为假），而界面是把
    // 整帧铺满视频区域的：不主动重画的话，旧比例的帧会被拉伸到新区域，
    // 看起来就是「切播放列表时画面变形」。这里记一笔，下一轮无条件重画一次。
    let mut resized = false;
    // 还没有任何画面（刚启动、纯音频）时不重画：那会送出一张纯黑图，
    // 把界面上的「拖入文件」空状态盖掉。
    let mut rendered_once = false;

    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }

        // 等「有新帧」或「尺寸变了」。
        signal.wait(Duration::from_millis(40));

        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }

        let target = *shared.target.lock().unwrap_or_else(|e| e.into_inner());
        if target.width < 2 || target.height < 2 {
            continue;
        }
        if target != current {
            current = target;
            buffer.resize(current.width, current.height);
            resized = true;
        }

        // mpv 告诉我们有帧才渲染；没有就继续等，避免空转。
        // 但两种「界面主动要求」要例外：暂停时这就是唯一能让画面重新适配的机会。
        //   * 尺寸变了（拖窗口、开关播放列表）
        //   * 换滤镜/比例这类只改画法、不改尺寸的设置（会走 request_redraw）
        let forced = shared.redraw.swap(false, Ordering::SeqCst) && rendered_once;
        let flags = unsafe { (api.render_context_update)(context) };
        let fresh = flags & ffi::RENDER_UPDATE_FRAME != 0;
        if !fresh && !forced && !(resized && rendered_once) {
            continue;
        }

        // 界面还没取走上一帧，就丢掉这一帧。播放器宁可掉帧也不能让内存堆积。
        // `resized` 不清，下一轮还会重试，尺寸变化不会因为这次丢掉而失效。
        if shared.frame_pending.swap(true, Ordering::SeqCst) {
            continue;
        }

        let rendered = frame::render_sw(&api, context, current.width, current.height, &mut buffer);
        if let Err(error) = rendered {
            log_warn!("渲染帧失败: {error:#}");
            shared.frame_pending.store(false, Ordering::SeqCst);
            continue;
        }

        let image = buffer.to_image(current.rotation);
        shared.frame_serial.fetch_add(1, Ordering::SeqCst);
        shared.notify(PlayerEvent::Frame(Arc::new(image)));
        unsafe { (api.render_context_report_swap)(context) };
        resized = false;
        rendered_once = true;
    }

    unsafe {
        (api.render_context_set_update_callback)(context, None, std::ptr::null_mut());
        (api.render_context_free)(context);
    }
}
