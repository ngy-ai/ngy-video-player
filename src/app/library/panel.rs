//! 素材库的会话状态、导入进度与缩略图工作线程。

use super::super::*;
use crate::timeline::{AssetId, AssetState};
use gpui_kit::Subscription;
use gpui_kit::component::input::InputState;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// 一张素材缩略图的状态。
pub(crate) enum ThumbState {
    Ready(Arc<RenderImage>),
    Failed,
}

/// 素材库面板的会话状态。关掉面板就整个丢掉，下次打开重新来。
pub(crate) struct LibraryPanel {
    /// 搜索框。文件名多是中文，自绘输入框没有输入法。
    pub(crate) search: Entity<InputState>,
    /// 搜索框当前的文本。输入框只会重绘自己，靠观察者同步到这里来过滤列表。
    pub(crate) query: String,
    /// 选中的素材。移除按钮与拖拽都作用在整组上。
    pub(crate) selected: BTreeSet<AssetId>,
    /// 上一次点过的行。Shift 连选从它开始算区间。
    pub(crate) anchor: Option<AssetId>,
    /// 已经生成好的缩略图。
    pub(crate) thumbs: HashMap<AssetId, ThumbState>,
    /// 已经投递、还没回来的缩略图请求。
    pub(crate) requested: HashSet<AssetId>,
    /// 素材状态的缓存：`fs::metadata` 不能每帧都调。
    pub(crate) states: HashMap<AssetId, AssetState>,
    /// 「比序列大、还没有代理」的素材。补代理按钮数它，判断要 stat 目录，
    /// 只在刷新状态时算一次，别放到每帧的绘制里。
    pub(crate) missing_proxy: Vec<AssetId>,
    /// 串行生成缩略图的后台线程。
    pub(crate) worker: ThumbWorker,
    pub(crate) _search_changes: Subscription,
}

/// 一次批量导入的进度。导入完成就清空。
pub(crate) struct LibraryImport {
    pub(crate) total: usize,
    pub(crate) done: usize,
    pub(crate) failed: usize,
}

impl LibraryImport {
    pub(crate) fn finished(&self) -> bool {
        self.done >= self.total
    }
}

pub(crate) enum ThumbRequest {
    Cover(AssetId, PathBuf),
    Shutdown,
}

/// 缩略图工作线程的句柄。
///
/// 不 join：一张 100 秒的图块要跑几秒 ffmpeg，关面板时等它等于把界面冻住。
/// 线程收到 `Shutdown` 后跑完手上这一张就退，进程退出时剩下的由系统回收。
pub(crate) struct ThumbWorker {
    requests: std::sync::mpsc::Sender<ThumbRequest>,
}

impl ThumbWorker {
    pub(crate) fn spawn(
        events: futures::channel::mpsc::UnboundedSender<(AssetId, Option<Arc<RenderImage>>)>,
    ) -> Self {
        let (requests, receiver) = std::sync::mpsc::channel::<ThumbRequest>();
        let spawned = std::thread::Builder::new()
            .name("library-thumb".into())
            .spawn(move || {
                for request in receiver {
                    match request {
                        ThumbRequest::Shutdown => break,
                        ThumbRequest::Cover(id, path) => {
                            let image = super::covers::render_cover(&path).ok().flatten();
                            // 主线程那边已经关了工程：没人收就退出。
                            if events.unbounded_send((id, image)).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        if let Err(error) = spawned {
            // 缩略图是锦上添花：起不来就退化成「没有封面」，不影响导入与剪辑。
            log_warn!("素材库缩略图线程启动失败: {error}");
        }
        Self { requests }
    }

    pub(crate) fn request(&self, id: AssetId, path: PathBuf) {
        let _ = self.requests.send(ThumbRequest::Cover(id, path));
    }
}

impl Drop for ThumbWorker {
    fn drop(&mut self) {
        let _ = self.requests.send(ThumbRequest::Shutdown);
    }
}
