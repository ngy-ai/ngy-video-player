//! 片段参数类编辑：倍率、速度曲线、片尾定格与调色。
//!
//! 这里只做「能不能改」这一层校验，夹取与波纹让位分别落在
//! `timeline::set_speed` / `timeline::set_remap` / `timeline::set_hold` 里。值没变时
//! 序列与原样逐字段相等，界面据此不落撤销。

use super::{ColorGrade, EditError, Id, Sequence, SpeedCurve, Tick, locate};

/// 改倍率。夹取与波纹让位都在 [`crate::timeline::set_speed`] 里，这里只做前置校验。
///
/// 值没变不算错误（界面上点同一个档位很常见），返回 `Ok` 但序列原样。
pub(super) fn set_speed(sequence: &mut Sequence, clip_id: Id, speed: f32) -> Result<(), EditError> {
    if !speed.is_finite() {
        return Err(EditError::BadRate);
    }
    locate(sequence, clip_id)?;
    crate::timeline::set_speed(sequence, clip_id, speed);
    Ok(())
}

/// 换整条速度曲线。空曲线 = 回到常量倍率 `speed`。
///
/// 曲线里的点由界面算好（点一个就提交一整条）：单个点的增删在界面层做，
/// 这里只负责校验锁轨与落一步撤销，和调色同一个模式。
pub(super) fn set_remap(
    sequence: &mut Sequence,
    clip_id: Id,
    curve: &SpeedCurve,
) -> Result<(), EditError> {
    if curve
        .points
        .iter()
        .any(|point| !point.rate.is_finite() || !point.at.is_finite())
    {
        return Err(EditError::BadRate);
    }
    locate(sequence, clip_id)?;
    crate::timeline::set_remap(sequence, clip_id, curve);
    Ok(())
}

/// 加 / 取消片尾定格。负数定格没有意义：取消要显式给 `Tick::ZERO`。
pub(super) fn set_hold(sequence: &mut Sequence, clip_id: Id, hold: Tick) -> Result<(), EditError> {
    if hold.is_negative() {
        return Err(EditError::BadRate);
    }
    locate(sequence, clip_id)?;
    crate::timeline::set_hold(sequence, clip_id, hold);
    Ok(())
}

/// 改调色。值没变时序列与原样相等，界面据此不落撤销。
pub(super) fn set_color(
    sequence: &mut Sequence,
    clip_id: Id,
    color: &ColorGrade,
) -> Result<(), EditError> {
    let (track, index) = locate(sequence, clip_id)?;
    sequence.tracks[track].clips[index].color = color.clone();
    Ok(())
}
