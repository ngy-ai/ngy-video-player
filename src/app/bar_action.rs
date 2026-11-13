//! 菜单栏的一项动作。
//!
//! 从 `app.rs` 挪出来的：类型本身和 `PlayerApp` 的状态无关，只是「点了菜单之后
//! 要做什么」的一份清单，独立成文件后主文件的字段区不再被它挤占。

use super::*;

/// 菜单动作必须是 `Clone`：点击回调是 `Fn`，可能被多次调用，不能把动作 move 走。
#[derive(Clone)]
pub(super) enum BarAction {
    Command(Command),
    OpenFile,
    OpenFolder,
    /// 新建一个空工程，进入「预览 + 时间线」模式。
    NewProject,
    /// 打开 `.ngyproj`。
    OpenProject,
    /// 直接打开最近用过的某个工程。
    OpenRecentProject(PathBuf),
    /// 选媒体文件插进时间线。
    ImportToTimeline,
    /// 开关工程左侧的素材库面板。
    ToggleLibrary,
    /// 把文件 / 文件夹收进素材库（不碰时间线）。
    ImportAssets,
    ImportAssetFolder,
    /// 把当前播放列表里的文件按顺序插进时间线。
    ImportPlaylistToTimeline,
    /// 保存到当前路径；没存过时走「另存为」。
    Save,
    /// 换一个路径保存。
    SaveAs,
    /// 把素材复制到工程目录旁边，工程变成自包含的。
    CollectAssets,
    /// 切换工程预览区右侧的编辑历史侧栏，一步跳到任意操作。
    ShowHistory,
    /// 改当前序列的分辨率与帧率。
    SequenceSettings,
    /// 管理工程里的多条序列：新建 / 切换 / 改名 / 删除。
    ManageSequences,
    /// 退出工程模式，回到纯播放器。
    CloseProject,
    // -- 剪辑模式专用 ------------------------------------------------------
    /// 预览播放 / 暂停：走时间线时钟，不能直接发给 mpv。
    TogglePreview,
    /// 打开「跳到时间码」面板（`Ctrl`/`⌘` + `G`）。
    JumpToTimecode,
    /// 把当前序列导出成视频文件。
    ExportSequence,
    /// 把当前序列导出成 OpenTimelineIO 的 `.otio`。
    ExportOtio,
    /// 从一份 `.otio` 建一条新序列。
    ImportOtio,
    /// 撤销 / 重做一步编辑。
    Undo,
    Redo,
    /// 在播放头处切开片段、删掉选中片段。
    SplitClip,
    DeleteClips,
    /// 播放头附近加 / 删标记，设工作区入 / 出点。
    ToggleMarkerAtPlayhead,
    MarkIn,
    MarkOut,
    /// 各加一条视频 / 音频 / 字幕轨。
    AddVideoTrack,
    AddAudioTrack,
    AddSubtitleTrack,
    /// 在播放头处编辑一条新字幕；没有字幕轨时确定时自动补一条。
    AddSubtitle,
    LoadSubtitle,
    Previous,
    Next,
    Seek(f64),
    SetLoop(LoopMode),
    SetOnTop(OnTop),
    TogglePlaylist,
    ToggleFullscreen,
    ToggleMini,
    ToggleHelp,
    /// 打开设置窗口；`Some` 表示直接切到那一页。
    OpenSettings(Option<SettingsTab>),
    TakeScreenshot,
    Trim,
    /// 转成别的格式：当前文件，或整个文件夹。
    Convert,
    /// 标记 / 清除 A-B 循环。
    MarkAbStart,
    MarkAbEnd,
    ClearAbLoop,
    /// 字幕延迟（秒）：正数延后、负数提前、0 归零。
    SubtitleDelay(f64),
    /// 换掉 / 删掉内置音轨。
    EditAudio,
    /// 把外部字幕内嵌进容器。
    EditSubtitle,
    /// 编辑章节与容器元数据。
    EditMetadata,
    /// 生成代理文件。
    MakeProxy,
    /// 查看当前帧的示波器（亮度直方图 / 波形 / 矢量）。
    ShowScope,
    /// 混音台：每条音频轨的推子、声像与音量自动化。
    ShowMixer,
    /// 检测整片的镜头切换点。
    ShowScenes,
    /// 双机位并排导出。
    MulticamExport,
    /// 把当前播放列表拼成一个文件。
    ConcatExport,
    SetAsDefaultPlayer,
    OpenDefaultApps,
    RemoveAssociations,
    ToggleMaintainAssociations,
    About,
    Quit,
}
