//! 片段级关键帧：不透明度、缩放、位置随时间变化。
//!
//! 关键帧的时间是**片段内相对时间**（0 = 片段起点），不是序列时间：片段往后拖、
//! 被切开重排，动画都还贴着画面本身，不用跟着改一遍。
//!
//! 每个属性各存一条曲线，互不影响：只想推近就不要被迫也给不透明度打两个点。
//! 一条曲线里 0 个点表示「用静态默认值」，1 个点是「整段保持这个值」，
//! 2 个点以上才在点之间插值，两端之外保持端点值（不外推）。
//!
//! 导出侧的表达式用 `on`（输出帧号）换算时间：`zoompan` / `fade` 都拿不到
//! `t`，只有帧号，所以插值表达式里一律写 `(on/fps)`。

use crate::timeline::{Sequence, Tick};
use serde::{Deserialize, Serialize};

/// 能打关键帧的属性。存进工程的是字符串，认不出来的落进 `Unknown`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum Property {
    /// 不透明度 0–1。导出走 `fade`，只认「由透明变实」或反过来。
    Opacity,
    /// 缩放 0.1–4。导出走 `zoompan`，以画面中心为锚点。
    #[default]
    Scale,
    /// 水平位置 -1–1（相对画面宽度的比例，正数往右）。
    PositionX,
    /// 垂直位置 -1–1（正数往下）。
    PositionY,
    /// 工程里有但这一版不认识的属性。
    Unknown,
}

impl Property {
    pub const ALL: [Property; 4] = [
        Property::Scale,
        Property::PositionX,
        Property::PositionY,
        Property::Opacity,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Property::Opacity => "opacity",
            Property::Scale => "scale",
            Property::PositionX => "position_x",
            Property::PositionY => "position_y",
            Property::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Property::Opacity => "不透明度",
            Property::Scale => "缩放",
            Property::PositionX => "水平位置",
            Property::PositionY => "垂直位置",
            Property::Unknown => "未知属性",
        }
    }

    /// 没有关键帧时的取值。
    pub fn default_value(self) -> f32 {
        match self {
            Property::Opacity | Property::Scale => 1.0,
            _ => 0.0,
        }
    }

    pub fn min(self) -> f32 {
        match self {
            Property::Opacity => 0.0,
            Property::Scale => 0.1,
            _ => -1.0,
        }
    }

    pub fn max(self) -> f32 {
        match self {
            Property::Opacity => 1.0,
            Property::Scale => 4.0,
            _ => 1.0,
        }
    }

    /// 面板上调节时的步进。
    pub fn step(self) -> f32 {
        match self {
            Property::Opacity => 0.1,
            Property::Scale => 0.1,
            _ => 0.05,
        }
    }

    /// 展示用的格式化：百分比 / 倍数 / 比例偏移。
    pub fn format(self, value: f32) -> String {
        match self {
            Property::Opacity => format!("{:.0}%", value * 100.0),
            Property::Scale => format!("{value:.2}×"),
            Property::Unknown => format!("{value:.2}"),
            _ => format!("{:.0}%", value * 100.0),
        }
    }

    fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|property| property.id() == id)
            .unwrap_or(Property::Unknown)
    }
}

impl From<Property> for String {
    fn from(property: Property) -> Self {
        property.id().to_string()
    }
}

impl From<String> for Property {
    fn from(id: String) -> Self {
        Property::from_id(&id)
    }
}

/// 两个关键帧之间怎么走。
///
/// `Bezier` 是 CSS 那套 `cubic-bezier(x1, y1, x2, y2)` 的**缓动函数**：两条控制点都在
/// 单位方格内，`x` 是时间进度、`y` 是取值进度。控制点由界面上的曲线编辑器拖，
/// 预设见 [`Interpolation::PRESETS`]。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// 匀速：推近时速度是恒定的。
    #[default]
    Linear,
    /// 两头慢中间快：起手与收尾都软一点，看起来更自然。
    EaseInOut,
    /// 自定义三次贝塞尔。`x1/x2` 会被夹到 0–1（否则时间可能倒流），`y` 留在 0–1。
    Bezier { x1: f32, y1: f32, x2: f32, y2: f32 },
}

/// 曲线编辑器的预设：点一下就把控制点摆成这样。
///
/// 前两个是模型里原有的两档，后面三个是新的贝塞尔，编辑器只认这张表。
pub const EASING_PRESETS: [(&str, Interpolation); 5] = [
    ("线性", Interpolation::Linear),
    (
        "缓入缓出",
        Interpolation::Bezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        },
    ),
    (
        "缓入",
        Interpolation::Bezier {
            x1: 0.42,
            y1: 0.0,
            x2: 1.0,
            y2: 1.0,
        },
    ),
    (
        "缓出",
        Interpolation::Bezier {
            x1: 0.0,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        },
    ),
    (
        "强缓动",
        Interpolation::Bezier {
            x1: 0.7,
            y1: 0.0,
            x2: 0.3,
            y2: 1.0,
        },
    ),
];

impl Interpolation {
    /// 面板上并排摆的那几个（不含自定义，自定义是编辑器里拖出来的）。
    pub const ALL: [Interpolation; 2] = [Interpolation::Linear, Interpolation::EaseInOut];

    pub fn label(self) -> &'static str {
        match self {
            Interpolation::Linear => "线性",
            Interpolation::EaseInOut => "缓动",
            Interpolation::Bezier { .. } => "曲线",
        }
    }

    pub fn is_bezier(self) -> bool {
        matches!(self, Interpolation::Bezier { .. })
    }

    /// 贝塞尔的两个控制点；非贝塞尔返回默认的中点控制点（让编辑器有个起点）。
    pub fn controls(self) -> (f32, f32, f32, f32) {
        match self {
            Interpolation::Bezier { x1, y1, x2, y2 } => (x1, y1, x2, y2),
            Interpolation::EaseInOut => (0.42, 0.0, 0.58, 1.0),
            Interpolation::Linear => (1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0),
        }
    }

    /// 拖控制点：先夹进合法范围（`x` 必须单调，`y` 不许跳出 0–1，免得画面越界）。
    pub fn with_controls(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        let x1 = x1.clamp(0.0, 1.0);
        let x2 = x2.clamp(0.0, 1.0);
        Interpolation::Bezier {
            x1,
            y1: y1.clamp(0.0, 1.0),
            x2,
            y2: y2.clamp(0.0, 1.0),
        }
    }

    /// 把 0–1 的进度曲成实际比例。
    pub fn ease(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Interpolation::Linear => t,
            // smoothstep：两端导数为 0，不会有「突然起步」的顿挫。
            Interpolation::EaseInOut => t * t * (3.0 - 2.0 * t),
            Interpolation::Bezier { x1, y1, x2, y2 } => bezier_y(t, x1, y1, x2, y2),
        }
    }
}

/// 三次贝塞尔的 `y` 轴取值：给定时间进度 `x`，先解出参数 `t`，再算 `y(t)`。
///
/// 用二分而不是牛顿：这个函数每帧每个属性都要调，二分 20 次在这个量级上足够准
/// （误差 < 1e-4），而且不会因为控制点贴着边界而发散。
fn bezier_y(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    // 单位方格上的三次贝塞尔：P0=(0,0)、P3=(1,1)。
    let curve = |a: f32, b: f32, t: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    let (mut low, mut high) = (0.0f32, 1.0f32);
    for _ in 0..20 {
        let middle = (low + high) * 0.5;
        if curve(x1, x2, middle) < x {
            low = middle;
        } else {
            high = middle;
        }
    }
    curve(y1, y2, (low + high) * 0.5)
}

/// 一个关键帧：片段内相对时间 + 取值，外加「从我这个点到下一个点怎么走」。
///
/// `easing` 为 `None` 时用整条曲线的 `interpolation`——老工程里只有曲线级插值，
/// 这样读出来行为不变；新拖出来的点会带上自己的缓动，导出的表达式也按点走。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub at: Tick,
    pub value: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easing: Option<Interpolation>,
}

impl Keyframe {
    pub fn new(at: Tick, value: f32) -> Self {
        Self {
            at,
            value,
            easing: None,
        }
    }

    /// 这一段实际用的缓动。
    pub fn easing_or(&self, fallback: Interpolation) -> Interpolation {
        self.easing.unwrap_or(fallback)
    }
}

/// 一条属性的曲线。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Animation {
    pub property: Property,
    pub interpolation: Interpolation,
    /// 按 `at` 升序，同一个 `at` 只留一个（后写的覆盖）。
    pub keys: Vec<Keyframe>,
}

impl Animation {
    pub fn new(property: Property) -> Self {
        Self {
            property,
            ..Self::default()
        }
    }

    /// 有没有真的在做动画（至少两个点）。
    pub fn animated(&self) -> bool {
        self.keys.len() >= 2
    }

    /// 某个时刻的取值。没打点用默认值，点外钳制到端点。
    pub fn value_at(&self, at: Tick) -> f32 {
        self.value_or(at, self.property.default_value())
    }

    /// 同上，但没打点时用调用方给的静态值。
    pub fn value_or(&self, at: Tick, fallback: f32) -> f32 {
        if self.keys.is_empty() {
            return fallback;
        }
        let first = self.keys[0];
        if at <= first.at {
            return first.value;
        }
        let last = self.keys[self.keys.len() - 1];
        if at >= last.at {
            return last.value;
        }
        let index = self
            .keys
            .partition_point(|key| key.at <= at)
            .saturating_sub(1);
        let from = self.keys[index];
        let to = self.keys[index + 1];
        let span = (to.at - from.at).micros();
        if span <= 0 {
            return to.value;
        }
        let progress = (at - from.at).micros() as f32 / span as f32;
        let eased = from.easing_or(self.interpolation).ease(progress);
        from.value + (to.value - from.value) * eased
    }

    /// 在 `at` 处记一个点。同一个时刻已有就覆盖，返回是否新增。
    pub fn insert(&mut self, at: Tick, value: f32) -> bool {
        let value = value.clamp(self.property.min(), self.property.max());
        if let Some(existing) = self.keys.iter_mut().find(|key| key.at == at) {
            existing.value = value;
            return false;
        }
        self.keys.push(Keyframe::new(at, value));
        self.keys.sort_by_key(|key| key.at);
        true
    }

    /// 删掉 `at` 处的点（时间差在 `tolerance` 以内也算同一个）。
    pub fn remove_near(&mut self, at: Tick, tolerance: Tick) -> bool {
        let before = self.keys.len();
        self.keys
            .retain(|key| (key.at - at).micros().abs() > tolerance.micros());
        self.keys.len() != before
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }
}

/// 片段上的所有曲线。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keyframes {
    pub curves: Vec<Animation>,
}

impl Keyframes {
    pub fn is_empty(&self) -> bool {
        self.curves.iter().all(|curve| curve.keys.is_empty())
    }

    pub fn curve(&self, property: Property) -> Option<&Animation> {
        self.curves.iter().find(|curve| curve.property == property)
    }

    pub fn curve_mut(&mut self, property: Property) -> Option<&mut Animation> {
        self.curves
            .iter_mut()
            .find(|curve| curve.property == property)
    }

    /// 取（必要时新建）某条曲线。
    pub fn ensure(&mut self, property: Property) -> &mut Animation {
        if !self.curves.iter().any(|curve| curve.property == property) {
            self.curves.push(Animation::new(property));
        }
        let index = self
            .curves
            .iter()
            .position(|curve| curve.property == property)
            .expect("刚插入过");
        &mut self.curves[index]
    }

    /// 某个属性在片段内某时刻的值。
    pub fn value(&self, property: Property, at: Tick) -> f32 {
        self.curve(property)
            .map(|curve| curve.value_at(at))
            .unwrap_or(property.default_value())
    }

    /// 把 `from` 时刻的点整体挪到 `to`，取值不变。返回是否真的动过。
    ///
    /// 时间线上一根标记代表「这个时刻的所有属性」，所以挪的时候要把所有曲线上的点
    /// 一起挪走：只挪一条会让同一个标记下面的曲线错位，看上去就像动画自己变了。
    /// 目标时刻已经有点时后写的覆盖，与 [`Animation::insert`] 的语义一致。
    pub fn move_marks(&mut self, from: Tick, to: Tick) -> bool {
        if from == to {
            return false;
        }
        let mut moved = false;
        for curve in &mut self.curves {
            let Some(index) = curve.keys.iter().position(|key| key.at == from) else {
                continue;
            };
            let value = curve.keys.remove(index).value;
            curve.insert(to, value);
            moved = true;
        }
        moved
    }

    /// 所有曲线里出现过的时刻（点上画的菱形用），升序去重。
    pub fn marks(&self) -> Vec<Tick> {
        let mut marks: Vec<Tick> = self
            .curves
            .iter()
            .flat_map(|curve| curve.keys.iter().map(|key| key.at))
            .collect();
        marks.sort();
        marks.dedup();
        marks
    }

    /// 删掉认得出来的曲线之外的东西（未知属性在界面上没法编辑，直接丢掉）。
    pub fn prune(&mut self) {
        self.curves
            .retain(|curve| curve.property != Property::Unknown);
    }

    /// 认不出来的属性有几个：加载工程时提示一句。
    pub fn unknown_count(&self) -> usize {
        self.curves
            .iter()
            .filter(|curve| curve.property == Property::Unknown)
            .count()
    }
}

/// 把一条曲线写成 `zoompan` 能用的表达式；`on` 是输出帧号。
///
/// 一个点都没有时返回 `None`（不插动画滤镜）；只有一个点时返回常量。
fn expr(curve: &Animation, fps: f64) -> Option<String> {
    if curve.keys.is_empty() {
        return None;
    }
    let t = |tick: Tick| format!("{:.6}", tick.secs());
    // 帧号换时间：`zoompan` 的表达式里没有 `t`，只有输出帧序号 `on`。
    let time = format!("(on/{fps:.6})");
    let last = curve.keys[curve.keys.len() - 1];
    if curve.keys.len() == 1 {
        return Some(format!("{:.6}", last.value));
    }
    // 从最后一段往前套 if：`if(lt(t,a1), 段1, if(lt(t,a2), 段2, 末值))`。
    let mut expr = format!("{:.6}", last.value);
    for pair in curve.keys.windows(2).rev() {
        let (from, to) = (pair[0], pair[1]);
        let span = (to.at - from.at).secs();
        if span <= 0.0 {
            continue;
        }
        let segment = segment_expr(&from, &to, span, curve.interpolation, &time);
        expr = format!("if(lt({time},{:.6}),{segment},{expr})", t(to.at));
    }
    Some(expr)
}

/// 贝塞尔折线采样段数。
///
/// ffmpeg 的表达式里解不了三次方程（没有循环、没有求解器），所以把曲线切成若干段
/// 直线。8 段在一段 1–2 秒的推近里肉眼看不出折角，误差远小于 1/255；预览走的是
/// 真曲线（[`Interpolation::ease`]），两边取的是同一组采样值，不会各算一套。
pub const BEZIER_STEPS: usize = 8;

/// 一段（`from` → `to`）的取值表达式。
fn segment_expr(
    from: &Keyframe,
    to: &Keyframe,
    span: f64,
    fallback: Interpolation,
    time: &str,
) -> String {
    let easing = from.easing_or(fallback);
    let delta = to.value - from.value;
    let t = |tick: Tick| format!("{:.6}", tick.secs());
    let base = t(from.at);
    match easing {
        Interpolation::Linear | Interpolation::EaseInOut => {
            let progress = format!("(({time}-{base})/{span:.6})");
            let weight = match easing {
                Interpolation::Linear => progress,
                // smoothstep，与 `Interpolation::ease` 同一条式子。
                Interpolation::EaseInOut => {
                    format!("(({p})*({p})*(3.0-2.0*({p})))", p = progress)
                }
                Interpolation::Bezier { .. } => unreachable!("外层已经分流"),
            };
            format!("{:.6}+({delta:.6})*{weight}", from.value)
        }
        Interpolation::Bezier { .. } => {
            let steps = BEZIER_STEPS as f32;
            let mut expr = format!("{:.6}", to.value);
            for step in (0..BEZIER_STEPS).rev() {
                let f0 = step as f32 / steps;
                let f1 = (step + 1) as f32 / steps;
                let v0 = from.value + delta * easing.ease(f0);
                let v1 = from.value + delta * easing.ease(f1);
                let start = from.at.secs() + span * f64::from(f0);
                let width = span * f64::from(f1 - f0);
                let weight = format!("(({time}-{start:.6})/{width:.6})");
                expr = format!(
                    "if(lt({time},{:.6}),{v0:.6}+({:.6})*{weight},{expr})",
                    from.at.secs() + span * f64::from(f1),
                    v1 - v0
                );
            }
            expr
        }
    }
}

/// 缩放与位置合成一个 `zoompan`：它输出固定尺寸，正好接在 pad 之后。
///
/// 没有缩放 / 位置曲线时返回空串，导出链上不多一个滤镜。
pub fn motion_filter(keyframes: &Keyframes, sequence: &Sequence) -> String {
    let scale = keyframes.curve(Property::Scale);
    let x = keyframes.curve(Property::PositionX);
    let y = keyframes.curve(Property::PositionY);
    if scale.is_none_or(|curve| curve.keys.is_empty())
        && x.is_none_or(|curve| curve.keys.is_empty())
        && y.is_none_or(|curve| curve.keys.is_empty())
    {
        return String::new();
    }

    let fps = crate::timeline::sequence_fps(sequence);
    if fps <= 0.0 {
        return String::new();
    }
    let empty = Animation::default();
    let zoom = expr(scale.unwrap_or(&empty), fps).unwrap_or_else(|| "1.0".to_string());
    // 位置是「相对画面宽/高的比例」，-1–1；zoompan 的 x/y 是左上角坐标，
    // 所以先取居中位置，再加上偏移（偏移按画布尺寸算，不随缩放走）。
    let offset_x = x
        .and_then(|curve| expr(curve, fps))
        .unwrap_or_else(|| "0.0".to_string());
    let offset_y = y
        .and_then(|curve| expr(curve, fps))
        .unwrap_or_else(|| "0.0".to_string());
    let size = format!("{}x{}", sequence.width, sequence.height);
    let rate = format!("{}/{}", sequence.fps.num(), sequence.fps.den());

    format!(
        "zoompan=z='{zoom}':x='(iw-iw/zoom)/2+({offset_x})*iw':\
         y='(ih-ih/zoom)/2+({offset_y})*ih':d=1:s={size}:fps={rate}"
    )
}

/// 不透明度写成 `fade` 段：只认「从透明变实」与「从实变透明」。
///
/// 任意曲线的通用实现要 `geq`，那是逐像素求值，几分钟的片子会慢到没法用；
/// 淡入淡出是九成场景，用 fade 串联就够了。多段交替（淡出再淡入）也能表达。
pub fn fade_filters(keyframes: &Keyframes, duration: Tick) -> Vec<String> {
    let Some(curve) = keyframes.curve(Property::Opacity) else {
        return Vec::new();
    };
    if curve.keys.len() < 2 {
        return Vec::new();
    }
    let mut filters = Vec::new();
    for pair in curve.keys.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let end = to.at.min(duration);
        let start = from.at.max(Tick::ZERO);
        let span = (end - start).secs();
        if span <= 0.0 {
            continue;
        }
        // 只在端点值接近 0 / 1 时才认：中间值 fade 表达不了，宁可不做也不错做。
        let (kind, ok) = if from.value <= 0.01 && to.value >= 0.99 {
            ("in", true)
        } else if from.value >= 0.99 && to.value <= 0.01 {
            ("out", true)
        } else {
            ("in", false)
        };
        if !ok {
            continue;
        }
        // 不带 `alpha=1`：导出链是 yuv420p，没有 alpha 通道，带 alpha 的 fade 什么也不做。
        // 所以这里的淡入淡出是「淡到黑」，多轨时下面那条轨也会被一起盖住（已知限制）。
        filters.push(format!("fade=t={kind}:st={:.6}:d={span:.6}", start.secs()));
    }
    filters
}

#[cfg(test)]
mod tests;
