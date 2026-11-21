//! egui 界面：主窗口（盘列表 + 排除 + 开始）、确认弹窗（目标 + 排除 + PIN）、进度视图。
//!
//! 界面只发 `Command`、只读状态快照；枚举与写入都在后台线程完成。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use egui::{Color32, RichText};

use crate::i18n::{Lang, Texts, Usage};
use crate::icons::{self, Icon};

use multi_burn::backend::windows::{
    eject_drive, open_path_external, pick_image_file, pick_save_file, WindowsBackendFactory,
    WindowsDeviceEnumerator, WindowsPlatform,
};
use multi_burn::{
    BurnControl, BurnOptions, BurnOutcome, BurnPlan, BurnSnapshot, BurnState, BurnTask,
    DeviceCandidate, DeviceEnumerator, DeviceId, DiskPhase, DiskProgress, DiskState, FileSink,
    ImageInfo, VerifyReport,
};

/// 主操作色。
const PRIMARY: Color32 = Color32::from_rgb(0x3D, 0x8B, 0xFF);

/// 暗色层级：窗口底 / 卡片 / 悬停 / 边框。
const WINDOW_BG: Color32 = Color32::from_rgb(0x16, 0x18, 0x1D);
const CARD: Color32 = Color32::from_rgb(0x1E, 0x21, 0x28);
const CARD_HOVER: Color32 = Color32::from_rgb(0x26, 0x2A, 0x33);
const BORDER: Color32 = Color32::from_rgb(0x2B, 0x30, 0x3B);
const TEXT_MAIN: Color32 = Color32::from_rgb(0xE6, 0xE8, 0xEB);
const TEXT_SECOND: Color32 = Color32::from_rgb(0x9B, 0xA3, 0xB0);
const TEXT_HINT: Color32 = Color32::from_rgb(0x6E, 0x76, 0x83);
const DANGER: Color32 = Color32::from_rgb(0xE0, 0x4C, 0x4C);
const SUCCESS: Color32 = Color32::from_rgb(0x4C, 0xB8, 0x6B);
const WARN: Color32 = Color32::from_rgb(0xE0, 0xAA, 0x3C);
/// 主色按钮禁用态。
const PRIMARY_DISABLED: Color32 = Color32::from_rgb(0x2A, 0x35, 0x50);
const PRIMARY_DISABLED_TEXT: Color32 = Color32::from_rgb(0x7C, 0x93, 0xC4);

/// 品牌 Logo（`assets/logo.rgba`：192×192 RGBA，由设计稿导出）。
const LOGO_RGBA: &[u8] = include_bytes!("../assets/logo.rgba");
const LOGO_EDGE: u32 = 192;
/// 窗口 / 任务栏图标（`assets/app_icon.rgba`：64×64 RGBA）。
const APP_ICON_RGBA: &[u8] = include_bytes!("../assets/app_icon.rgba");
const APP_ICON_EDGE: u32 = 64;

/// 颜色线性混合（`t` = 0 取 `a`，1 取 `b`），用于悬停 / 选中过渡。
fn mix_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

/// 悬停 / 选中状态的过渡进度（0..1），egui 会自动请求重绘。
fn anim_bool(ui: &egui::Ui, id: egui::Id, value: bool, seconds: f32) -> f32 {
    ui.ctx().animate_bool_with_time(id, value, seconds)
}

/// Primary：实心主色（开始写入、加载、扫描设备）。
fn primary_button(text: impl Into<String>, min_size: egui::Vec2) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).strong().color(Color32::WHITE))
        .fill(PRIMARY)
        .stroke(egui::Stroke::NONE)
        .min_size(min_size)
}

/// Secondary：透明底 + 描边（浏览）。
fn secondary_button(text: impl Into<String>, min_size: egui::Vec2) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(TEXT_MAIN))
        .fill(Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0x3A, 0x41, 0x50)))
        .min_size(min_size)
}

/// Ghost：无边框淡字（导出、复制、清空）。
fn ghost_button(text: impl Into<String>, color: Color32) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).color(color))
        .fill(Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
}

/// 带图标的按钮层级。
#[derive(Clone, Copy, PartialEq)]
enum BtnKind {
    Primary,
    Ghost,
    /// 破坏性操作：悬停时文字与底色转为警示红。
    GhostDanger,
}

/// IconPark 图标 + 文字的按钮：图标与文字整体作为一个可点击控件，
/// 宽度按内容计算，`min_width` 用于主操作按钮的固定宽度。
fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    text: &str,
    kind: BtnKind,
    min_width: f32,
) -> egui::Response {
    const ICON: f32 = 16.0;
    const GAP: f32 = 6.0;
    let (height, pad_x) = match kind {
        BtnKind::Primary => (CONTROL_H + 2.0, 12.0),
        _ => (26.0, 6.0),
    };
    let base_color = match kind {
        BtnKind::Primary => Color32::WHITE,
        _ => TEXT_SECOND,
    };
    let font = egui::FontId::proportional(15.0);
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), base_color);
    let natural = pad_x * 2.0 + ICON + GAP + galley.size().x;
    let width = natural.max(min_width);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

    let hovered = response.hovered();
    // 悬停底色 / 文字色 120ms 过渡，避免生硬跳变。
    let t = anim_bool(ui, response.id.with("hover"), hovered, 0.12);
    let (fill, color) = match kind {
        BtnKind::Primary => (
            mix_color(PRIMARY, Color32::from_rgb(0x5A, 0x9C, 0xFF), t),
            Color32::WHITE,
        ),
        BtnKind::Ghost => (
            mix_color(Color32::TRANSPARENT, CARD_HOVER, t),
            TEXT_SECOND,
        ),
        BtnKind::GhostDanger => (
            mix_color(Color32::TRANSPARENT, Color32::from_rgb(0x3B, 0x20, 0x24), t),
            mix_color(TEXT_SECOND, DANGER, t),
        ),
    };
    // 悬停变色时重新排版文字，否则沿用已排好的 galley。
    let galley = if color == base_color {
        galley
    } else {
        ui.painter().layout_no_wrap(text.to_owned(), font, color)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, fill);

    let content = ICON + GAP + galley.size().x;
    let start = rect.center().x - content * 0.5;
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(start + ICON * 0.5, rect.center().y),
        egui::vec2(ICON, ICON),
    );
    icons::paint(painter, icon_rect, icon, color);
    painter.galley(
        egui::pos2(start + ICON + GAP, rect.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 日志级别筛选胶囊：彩色圆点 + 文字，选中时实心主色。
fn filter_chip(ui: &mut egui::Ui, label: &str, dot: Color32, selected: bool) -> bool {
    const DOT: f32 = 6.0;
    const GAP: f32 = 6.0;
    let font = egui::FontId::proportional(14.0);
    // 未选中基准色；选中态在下面按过渡进度混到白色。
    let text_color = TEXT_SECOND;
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, text_color);
    let width = (10.0 * 2.0 + DOT + GAP + galley.size().x).max(50.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::click());
    // 选中 / 悬停过渡：底色、描边与文字一起渐变。
    let sel_t = anim_bool(ui, response.id.with("sel"), selected, 0.15);
    let hov_t = anim_bool(ui, response.id.with("hov"), response.hovered(), 0.12);
    let text_color = mix_color(text_color, Color32::WHITE, sel_t);
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(14.0),
        text_color,
    );

    let painter = ui.painter();
    let fill = mix_color(
        mix_color(Color32::TRANSPARENT, CARD_HOVER, hov_t),
        PRIMARY,
        sel_t,
    );
    if fill.a() > 0 {
        painter.rect_filled(rect, 13.0, fill);
    }
    if sel_t < 1.0 {
        let stroke_alpha = (BORDER.a() as f32 * (1.0 - sel_t)) as u8;
        painter.rect_stroke(
            rect,
            13.0,
            egui::Stroke::new(
                1.0_f32,
                Color32::from_rgba_premultiplied(
                    (BORDER.r() as f32 * (1.0 - sel_t)) as u8,
                    (BORDER.g() as f32 * (1.0 - sel_t)) as u8,
                    (BORDER.b() as f32 * (1.0 - sel_t)) as u8,
                    stroke_alpha,
                ),
            ),
        );
    }
    let content = DOT + GAP + galley.size().x;
    let start = rect.center().x - content * 0.5;
    // 选中态下主色蓝点会融进主色底，改成白点。
    let dot_color = if dot == PRIMARY {
        mix_color(dot, Color32::WHITE, sel_t)
    } else {
        dot
    };
    painter.circle_filled(
        egui::pos2(start + DOT * 0.5, rect.center().y),
        DOT * 0.5,
        dot_color,
    );
    painter.galley(
        egui::pos2(start + DOT + GAP, rect.center().y - galley.size().y * 0.5),
        galley,
        text_color,
    );
    response.clicked()
}

/// 状态徽章：图标 + 文字双编码，不只靠颜色区分成败。
fn status_badge(ui: &mut egui::Ui, ok: bool, text: &str) -> egui::Response {
    const HEIGHT: f32 = 24.0;
    const PAD: f32 = 10.0;
    const ICON: f32 = 13.0;
    const GAP: f32 = 6.0;
    let color = if ok { SUCCESS } else { DANGER };
    let fill = if ok {
        Color32::from_rgba_unmultiplied(0x4C, 0xB8, 0x6B, 30)
    } else {
        Color32::from_rgba_unmultiplied(0xE0, 0x4C, 0x4C, 30)
    };
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(12.5),
        color,
    );
    let width = PAD * 2.0 + ICON + GAP + galley.size().x;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, HEIGHT * 0.5, fill);
    icons::paint(
        painter,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + PAD + ICON * 0.5, rect.center().y),
            egui::vec2(ICON, ICON),
        ),
        if ok { Icon::Check } else { Icon::Close },
        color,
    );
    painter.galley(
        egui::pos2(
            rect.left() + PAD + ICON + GAP,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
    response
}

/// 纯图标按钮（展开箭头、行内重试等）。
fn icon_only_button(ui: &mut egui::Ui, icon: Icon, color: Color32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let t = anim_bool(ui, response.id.with("hover"), response.hovered(), 0.12);
    if t > 0.0 {
        ui.painter()
            .rect_filled(rect, 6.0, mix_color(Color32::TRANSPARENT, CARD_HOVER, t));
    }
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0)),
        icon,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 统计条里的一格：大数字 + 小标签。
fn stat_block(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(value).size(18.0).strong().color(color));
        ui.label(RichText::new(label).size(12.0).color(TEXT_SECOND));
    });
}

/// 从开始 / 结束时间算秒数（用于平均速度）。
fn elapsed_seconds(
    start: Option<chrono::DateTime<chrono::FixedOffset>>,
    end: Option<chrono::DateTime<chrono::FixedOffset>>,
) -> Option<f64> {
    let seconds = end?.signed_duration_since(start?).num_milliseconds() as f64 / 1000.0;
    (seconds > 0.05).then_some(seconds)
}

/// CSV 字段转义。
fn csv_field(text: &str) -> String {
    if text.contains(',') || text.contains('"') || text.contains('\n') {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// 历史记录导出格式。
#[derive(Clone, Copy)]
enum ExportKind {
    Csv,
    Json,
}

/// 启动界面；调用方负责把错误显示出来。
pub fn run() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 740.0])
            .with_min_inner_size([900.0, 650.0])
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "齐刻 Qike Flash",
        options,
        Box::new(|cc| {
            install_fonts_and_style(&cc.egui_ctx);
            let logo = load_logo(&cc.egui_ctx);
            Ok(Box::new(BurnApp::new(logo)))
        }),
    )
}

/// 把内嵌的 Logo 上传成纹理（只做一次）。
fn load_logo(ctx: &egui::Context) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [LOGO_EDGE as usize, LOGO_EDGE as usize],
        LOGO_RGBA,
    );
    ctx.load_texture("qike-logo", image, egui::TextureOptions::LINEAR)
}

/// 程序图标：品牌 Logo（白色圆角底 + 深色 U 盘线稿）。
fn app_icon() -> egui::IconData {
    egui::IconData {
        rgba: APP_ICON_RGBA.to_vec(),
        width: APP_ICON_EDGE,
        height: APP_ICON_EDGE,
    }
}

// ---------------------------------------------------------------- 字体

/// 界面字体：思源黑体（Noto Sans SC 可变字重 TTF，与 Adobe 思源黑体同源，
/// Google 发行版；SIL OFL 1.1）。内嵌进程序，不依赖系统已安装字体。
const UI_FONT: &[u8] = include_bytes!("../assets/NotoSansSC-VF.ttf");
const UI_FONT_NAME: &str = "source-han-sans";

/// 全界面统一使用思源黑体，并放大盘列表字号。
fn install_fonts_and_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        UI_FONT_NAME.to_owned(),
        egui::FontData::from_static(UI_FONT),
    );
    // 比例字体与等宽字体都以思源黑体为首选，缺失字形再回落到 egui 内置字体。
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, UI_FONT_NAME.to_owned());
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (egui::TextStyle::Heading, egui::FontId::new(23.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Body, egui::FontId::new(16.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Button, egui::FontId::new(16.0, egui::FontFamily::Proportional)),
        (egui::TextStyle::Small, egui::FontId::new(13.5, egui::FontFamily::Proportional)),
        (egui::TextStyle::Monospace, egui::FontId::new(15.0, egui::FontFamily::Monospace)),
    ]
    .into();
    // 设计令牌：圆角统一 8px，间距用 8/12/16/24 的倍数。
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 5.0);
    style.spacing.window_margin = egui::Margin::same(4.0);
    style.visuals.widgets.noninteractive.rounding = egui::Rounding::same(8.0);
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(8.0);
    style.visuals.widgets.hovered.rounding = egui::Rounding::same(8.0);
    style.visuals.widgets.active.rounding = egui::Rounding::same(8.0);
    style.visuals.widgets.open.rounding = egui::Rounding::same(8.0);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    // 暗色层次：窗口 / 卡片 / 输入框底 / 正文对比度。
    style.visuals.panel_fill = WINDOW_BG;
    style.visuals.window_fill = WINDOW_BG;
    style.visuals.faint_bg_color = CARD;
    style.visuals.extreme_bg_color = Color32::from_rgb(0x12, 0x14, 0x19);
    style.visuals.override_text_color = Some(TEXT_MAIN);
    style.visuals.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    style.visuals.selection.bg_fill = PRIMARY.gamma_multiply(0.45);
    // 常规按钮：透明底 + 边框；悬停加亮。
    style.visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(0x3A, 0x41, 0x50));
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, TEXT_MAIN);
    style.visuals.widgets.hovered.bg_fill = CARD_HOVER;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(0x4A, 0x52, 0x63));
    style.visuals.widgets.active.bg_fill = CARD_HOVER.gamma_multiply(0.8);
    ctx.set_style(style);
}

// ---------------------------------------------------------------- 视觉令牌

/// 内容区左右内边距：主视图、底部操作栏、日志面板统一用它，保证三条栏左边缘对齐。
const PAGE_X: f32 = 12.0;
const PAGE_Y: f32 = 10.0;
/// 常规控件高度，按钮、输入框同一基线。
const CONTROL_H: f32 = 30.0;

/// 产品版本（关于页展示）。
const APP_VERSION: &str = "0.0.1-α";

fn page_frame() -> egui::Frame {
    egui::Frame::none().inner_margin(egui::Margin::symmetric(PAGE_X, PAGE_Y))
}

/// 底部动作条：CentralPanel 已经给了左右 PAGE_X，这里只补纵向内边距，
/// 否则按钮会比镜像卡再缩进 12px；底部留 20px，避免按钮贴着窗口下沿。
fn bar_frame() -> egui::Frame {
    egui::Frame::none().inner_margin(egui::Margin {
        left: 0.0,
        right: 0.0,
        top: PAGE_Y,
        bottom: 20.0,
    })
}

// ---------------------------------------------------------------- 后台消息

struct StartRequest {
    image_path: PathBuf,
    targets: Vec<DeviceId>,
    excluded: Vec<DeviceId>,
    /// 写入后是否校验（来自设置）。
    verify: bool,
}

enum Command {
    Rescan,
    Inspect(PathBuf),
    Start(StartRequest),
    Cancel,
    SetWatch(bool),
}

enum Event {
    Devices(Vec<DeviceCandidate>),
    ImageLoaded(Box<ImageInfo>),
    ImageFailed(String),
    /// 后台线程的即时提示；文案在 UI 线程按当前语言翻译。
    Busy(BusyText),
    Started(Arc<BurnControl>),
    Finished(Result<BurnOutcome, BatchFail>),
}

/// 后台线程发来的即时提示（不带语言，UI 侧翻译）。
enum BusyText {
    Hashing,
    EnumerateFailed(String),
}

/// 后台批量写入的整体失败原因。
enum BatchFail {
    /// 内核返回的错误（技术性文案，保持原文）。
    Core(String),
    /// 台账文件不可用。
    Ledger(LedgerFail),
}

/// 台账失败的具体环节。
enum LedgerFail {
    Locate(String),
    LocatePlain,
    Create(String, std::io::Error),
    /// 第二项为 BurnError 的 Display 文本。
    Write(String, String),
}

impl LedgerFail {
    /// 翻译成当前语言的完整提示。
    fn text(self, lang: Lang) -> String {
        match self {
            Self::Locate(error) => lang.ledger_locate_failed(&error),
            Self::LocatePlain => lang.ledger_locate_failed_plain().to_owned(),
            Self::Create(path, error) => lang.ledger_create_failed(&path, &error.to_string()),
            Self::Write(path, error) => lang.ledger_write_failed(&path, &error),
        }
    }
}

fn spawn_worker() -> (Sender<Command>, Receiver<Event>) {
    let (command_tx, command_rx) = mpsc::channel::<Command>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();

    thread::spawn(move || {
        let platform = WindowsPlatform::new();
        let enumerator = platform.enumerator();
        let factory = platform.factory();
        let mut active: Option<Arc<BurnControl>> = None;
        // 自动检测：空闲时定期比对设备接口签名，变化才做完整枚举。
        let mut watch = true;
        let mut last_signature: Option<Vec<String>> = None;

        loop {
            let command = match command_rx.recv_timeout(Duration::from_millis(1200)) {
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !watch || active.is_some() {
                        continue;
                    }
                    let signature = match enumerator.interface_signature() {
                        Ok(value) => value,
                        Err(_) => continue,
                    };
                    if last_signature.as_ref() == Some(&signature) {
                        continue;
                    }
                    last_signature = Some(signature);
                    if let Ok(devices) = enumerator.enumerate() {
                        let _ = event_tx.send(Event::Devices(devices));
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            match command {
                Command::SetWatch(enabled) => watch = enabled,
                Command::Rescan => {
                    if let Ok(signature) = enumerator.interface_signature() {
                        last_signature = Some(signature);
                    }
                    match enumerator.enumerate() {
                        Ok(devices) => {
                            let _ = event_tx.send(Event::Devices(devices));
                        }
                        Err(error) => {
                            let _ = event_tx.send(Event::Devices(Vec::new()));
                            let _ = event_tx.send(Event::Busy(BusyText::EnumerateFailed(
                                error.to_string(),
                            )));
                        }
                    }
                }
                Command::Inspect(path) => match ImageInfo::from_path(&path) {
                    Ok(info) => {
                        let _ = event_tx.send(Event::ImageLoaded(Box::new(info)));
                    }
                    Err(error) => {
                        let _ = event_tx.send(Event::ImageFailed(error.to_string()));
                    }
                },
                Command::Cancel => {
                    if let Some(control) = active.take() {
                        control.cancel();
                    }
                }
                Command::Start(request) => {
                    let result = run_batch(&request, &enumerator, &factory, &event_tx, &mut active);
                    let _ = event_tx.send(Event::Finished(result));
                    if let Ok(signature) = enumerator.interface_signature() {
                        last_signature = Some(signature);
                    }
                }
            }
        }
    });

    (command_tx, event_rx)
}

fn run_batch(
    request: &StartRequest,
    enumerator: &WindowsDeviceEnumerator,
    factory: &WindowsBackendFactory,
    events: &Sender<Event>,
    active: &mut Option<Arc<BurnControl>>,
) -> Result<BurnOutcome, BatchFail> {
    let _ = events.send(Event::Busy(BusyText::Hashing));
    let info =
        ImageInfo::from_path(&request.image_path).map_err(|error| BatchFail::Core(error.to_string()))?;
    let image = info.open().map_err(|error| BatchFail::Core(error.to_string()))?;

    let ledger = ledger_path().map_err(BatchFail::Ledger)?;
    let mut sink = FileSink::create(&ledger).map_err(|error| {
        BatchFail::Ledger(LedgerFail::Write(ledger.display().to_string(), error.to_string()))
    })?;

    let plan = BurnPlan::new(
        image,
        info.name.clone(),
        request.image_path.to_string_lossy().into_owned(),
        info.sha256.clone(),
        request.targets.clone(),
        request.excluded.clone(),
        BurnOptions {
            verify: request.verify,
            ..BurnOptions::default()
        },
    );
    let mut task = BurnTask::new(plan, enumerator, factory, &mut sink);
    let control = task.control();
    *active = Some(Arc::clone(&control));
    let _ = events.send(Event::Started(control));

    let result = task.run().map_err(|error| BatchFail::Core(error.to_string()));
    *active = None;
    result
}

/// 历史日志文件在安装目录的 logs 文件夹下。
fn log_file_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("multi-burn.log"))
}

/// 日志目录（安装目录下的 logs 文件夹）。
fn logs_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("logs"))
}

/// 台账目录：安装目录下的 history 文件夹。
fn ledger_path() -> Result<PathBuf, LedgerFail> {
    let exe = std::env::current_exe().map_err(|error| LedgerFail::Locate(error.to_string()))?;
    let dir = exe
        .parent()
        .ok_or(LedgerFail::LocatePlain)?
        .join("history");
    std::fs::create_dir_all(&dir)
        .map_err(|error| LedgerFail::Create(dir.display().to_string(), error))?;
    Ok(dir.join("ledger.jsonl"))
}

/// 旧版台账放在 exe 根目录，升级后首次启动时搬进 history 文件夹。
fn migrate_ledger() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(root) = exe.parent() else {
        return;
    };
    let old = root.join("ledger.jsonl");
    if !old.exists() {
        return;
    }
    let dir = root.join("history");
    let new = dir.join("ledger.jsonl");
    if new.exists() {
        return;
    }
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::rename(&old, &new);
    }
}

// ---------------------------------------------------------------- 应用状态

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Main,
    Progress,
    Done,
}

/// 左侧导航的五个页面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Logs,
    History,
    Settings,
    About,
}

impl Page {
    fn label(self, t: &Texts) -> &'static str {
        match self {
            Page::Home => t.nav_home,
            Page::Logs => t.nav_logs,
            Page::History => t.nav_history,
            Page::Settings => t.nav_settings,
            Page::About => t.nav_about,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Page::Home => Icon::Home,
            Page::Logs => Icon::Log,
            Page::History => Icon::History,
            Page::Settings => Icon::Setting,
            Page::About => Icon::Info,
        }
    }
}

/// 日志页筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFilter {
    All,
    Info,
    Warn,
    Error,
}

impl LogFilter {
    fn label(self, t: &Texts) -> &'static str {
        match self {
            LogFilter::All => t.filter_all,
            LogFilter::Info => t.filter_info,
            LogFilter::Warn => t.filter_warn,
            LogFilter::Error => t.filter_error,
        }
    }

    fn accepts(self, level: LogLevel) -> bool {
        match self {
            LogFilter::All => true,
            LogFilter::Info => matches!(level, LogLevel::Info | LogLevel::Ok),
            LogFilter::Warn => level == LogLevel::Warn,
            LogFilter::Error => level == LogLevel::Error,
        }
    }

    /// 胶囊上的级别色点，与日志行左侧圆点一致。
    fn dot_color(self) -> Color32 {
        match self {
            LogFilter::All => TEXT_SECOND,
            LogFilter::Info => PRIMARY,
            LogFilter::Warn => WARN,
            LogFilter::Error => DANGER,
        }
    }
}

/// 用户设置，持久化到 exe 同目录的 settings.json。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    auto_detect: bool,
    verify: bool,
    eject_after: bool,
    /// 日志保留天数，0 = 永久。
    log_keep_days: u32,
    /// 界面语言。
    lang: Lang,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_detect: true,
            verify: true,
            eject_after: false,
            log_keep_days: 30,
            lang: Lang::default(),
        }
    }
}

/// 台账里的一条批次记录。
struct LedgerRecord {
    at: String,
    image_name: String,
    /// 镜像完整路径（旧台账没有该字段时为空串）。
    image_path: String,
    image_size: u64,
    total: usize,
    succeeded: usize,
    excluded: usize,
    /// 批次开始时间与最后一盘结束时间，用来算用时。
    started: Option<chrono::DateTime<chrono::FixedOffset>>,
    ended: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// 逐盘结果。
    disks: Vec<DiskRecord>,
    /// 原始 JSON 行（批次行 + 逐盘行），删除记录时按原样回写。
    raw: Vec<String>,
}

impl LedgerRecord {
    /// 跨刷新稳定的标识（用于展开态与多选）。
    fn key(&self) -> String {
        format!("{}|{}", self.at, self.image_name)
    }

    fn all_ok(&self) -> bool {
        self.total > 0 && self.succeeded == self.total
    }

    /// 失败盘的短标签（盘符优先，其次设备 ID），用于状态徽章。
    fn failed_labels(&self) -> Vec<String> {
        self.disks
            .iter()
            .filter(|disk| !disk.ok)
            .map(|disk| {
                if disk.letters.trim().is_empty() {
                    short_device(&disk.device)
                } else {
                    disk.letters.split_whitespace().next().map(str::to_string).unwrap_or_default()
                }
            })
            .filter(|label| !label.is_empty())
            .collect()
    }

    fn first_error(&self) -> Option<&str> {
        self.disks
            .iter()
            .filter(|disk| !disk.ok)
            .find_map(|disk| disk.error.as_deref())
            .filter(|text| !text.trim().is_empty())
    }
}

/// 台账里的一块盘。
struct DiskRecord {
    device: String,
    /// 厂商 + 型号（旧台账可能为空）。
    model: String,
    /// 写入前的盘符（如 `F: G:`）。
    letters: String,
    capacity: u64,
    write: String,
    verify: String,
    /// 失败原因原始文案。
    error: Option<String>,
    ok: bool,
}

/// 历史页状态筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LedgerFilter {
    All,
    Ok,
    Failed,
}

impl LedgerFilter {
    fn label(self, t: &Texts) -> &'static str {
        match self {
            Self::All => t.filter_all,
            Self::Ok => t.filter_success,
            Self::Failed => t.filter_failed,
        }
    }

    fn matches(self, record: &LedgerRecord) -> bool {
        match self {
            Self::All => true,
            Self::Ok => record.all_ok(),
            Self::Failed => !record.all_ok(),
        }
    }
}

/// 历史页行内可触发的动作（在遍历结束后统一应用，避免借用冲突）。
enum LedgerAction {
    Toggle(String),
    Select(String),
    Delete(String),
    Retry(String),
    ViewLog(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Info,
    Ok,
    Warn,
    Error,
}

impl LogLevel {
    fn tag(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Ok => "OK",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

struct LogLine {
    time: String,
    level: LogLevel,
    text: String,
}

struct BurnApp {
    devices: Vec<DeviceCandidate>,
    excluded: HashSet<String>,
    image_path: String,
    image_info: Option<ImageInfo>,
    image_error: Option<String>,
    loading_image: bool,
    scanning: bool,
    stage: Stage,
    confirm_open: bool,
    /// 确认弹窗打开时冻结的参与集合（D4：弹窗期间新插入的盘不参与）。
    pending: Option<Vec<String>>,
    pin_focus: bool,
    pin_expected: String,
    pin_input: String,
    control: Option<Arc<BurnControl>>,
    outcome: Option<BurnOutcome>,
    notice: Option<String>,
    logs: Vec<LogLine>,
    /// 当前导航页。
    page: Page,
    /// 上一帧的导航页（用于切换淡入）。
    last_page: Page,
    /// 进入当前页的时刻（用于切换淡入）。
    page_shown_at: Instant,
    /// 日志页筛选。
    log_filter: LogFilter,
    /// 日志页搜索关键字。
    log_query: String,
    /// 日志页是否自动滚动到底部（用户上翻后暂停）。
    log_autoscroll: bool,
    /// 日志页「清空」的两段式确认。
    log_clear_arm: bool,
    /// 台账记录（历史页）。
    ledger: Vec<LedgerRecord>,
    /// 历史页展开的记录（稳定的 key）。
    ledger_open: Option<String>,
    /// 历史页多选集合。
    ledger_selected: HashSet<String>,
    /// 历史页搜索关键字。
    ledger_query: String,
    /// 历史页状态筛选。
    ledger_filter: LedgerFilter,
    /// 历史页「清空」的两段式确认。
    ledger_clear_arm: bool,
    /// 清空记录时是否一并清空日志。
    ledger_clear_logs: bool,
    /// 历史页「删除所选」的二次确认。
    ledger_delete_arm: bool,
    /// 历史页自动刷新计时（写入进行中）。
    ledger_refresh_at: Instant,
    /// 用户设置。
    settings: Settings,
    /// 品牌 Logo 纹理。
    logo: egui::TextureHandle,
    /// 本次写入的目标盘符，用于完成后弹出。
    burn_letters: Vec<char>,
    /// 实时日志文件路径（logs 目录下）。
    log_file: Option<PathBuf>,
    auto_watch: bool,
    last_sample: Instant,
    prev_bytes: HashMap<String, u64>,
    speeds: HashMap<String, f64>,
    commands: Sender<Command>,
    events: Receiver<Event>,
}

impl BurnApp {
    fn new(logo: egui::TextureHandle) -> Self {
        let (commands, events) = spawn_worker();
        let settings = load_settings();
        let lang = settings.lang;
        let _ = commands.send(Command::SetWatch(settings.auto_detect));
        prune_logs(settings.log_keep_days);
        migrate_ledger();
        let _ = commands.send(Command::Rescan);
        let mut app = Self {
            devices: Vec::new(),
            excluded: HashSet::new(),
            image_path: String::new(),
            image_info: None,
            image_error: None,
            loading_image: false,
            scanning: true,
            stage: Stage::Main,
            confirm_open: false,
            pending: None,
            pin_focus: false,
            pin_expected: String::new(),
            pin_input: String::new(),
            control: None,
            outcome: None,
            notice: None,
            logs: Vec::new(),
            page: Page::Home,
            last_page: Page::Home,
            page_shown_at: Instant::now(),
            log_filter: LogFilter::All,
            log_query: String::new(),
            log_autoscroll: true,
            log_clear_arm: false,
            ledger: Vec::new(),
            ledger_open: None,
            ledger_selected: HashSet::new(),
            ledger_query: String::new(),
            ledger_filter: LedgerFilter::All,
            ledger_clear_arm: false,
            ledger_clear_logs: false,
            ledger_delete_arm: false,
            ledger_refresh_at: Instant::now(),
            auto_watch: settings.auto_detect,
            settings,
            logo,
            burn_letters: Vec::new(),
            last_sample: Instant::now(),
            prev_bytes: HashMap::new(),
            speeds: HashMap::new(),
            log_file: log_file_path(),
            commands,
            events,
        };
        app.load_ledger();
        app.log(lang.log_started());
        app
    }

    /// 当前界面语言（快捷取用）。
    fn lang(&self) -> Lang {
        self.settings.lang
    }

    /// 追加一条带时间戳的日志，最多保留 500 条。
    fn log(&mut self, text: impl std::fmt::Display) {
        self.push_log(LogLevel::Info, text);
    }

    fn log_ok(&mut self, text: impl std::fmt::Display) {
        self.push_log(LogLevel::Ok, text);
    }

    fn log_warn(&mut self, text: impl std::fmt::Display) {
        self.push_log(LogLevel::Warn, text);
    }

    fn log_error(&mut self, text: impl std::fmt::Display) {
        self.push_log(LogLevel::Error, text);
    }

    fn push_log(&mut self, level: LogLevel, text: impl std::fmt::Display) {
        let now = chrono::Local::now();
        let time = now.format("%m-%d %H:%M:%S").to_string();
        let text = text.to_string();
        // 同步追加一行到历史日志文件（尽力而为，失败不影响界面）。
        if let Some(path) = &self.log_file {
            use std::io::Write;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let line = format!(
                "{} [{}] {}\n",
                now.format("%Y-%m-%d %H:%M:%S"),
                level.tag(),
                text
            );
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
            {
                let _ = file.write_all(line.as_bytes());
            }
        }
        self.logs.push(LogLine { time, level, text });
        if self.logs.len() > 500 {
            let overflow = self.logs.len() - 500;
            self.logs.drain(..overflow);
        }
    }

    /// 日志纯文本（供复制）。
    fn logs_as_text(&self) -> String {
        self.logs
            .iter()
            .map(|line| format!("[{}] {}", line.time, line.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn drain_events(&mut self) {
        loop {
            match self.events.try_recv() {
                Ok(Event::Devices(devices)) => {
                    // 确认期间冻结设备列表：弹窗里没确认过的盘绝不参与（D2、D4）。
                    if self.confirm_open {
                        continue;
                    }
                    let lang = self.lang();
                    self.notice = None;
                    let count = devices.len();
                    self.devices = devices;
                    // C7：排除不跨插拔记忆。
                    self.excluded.clear();
                    self.scanning = false;
                    if count == 0 {
                        self.log(lang.scan_none());
                    } else {
                        self.log(lang.scan_done(count));
                    }
                }
                Ok(Event::ImageLoaded(info)) => {
                    let lang = self.lang();
                    self.loading_image = false;
                    self.image_error = None;
                    self.notice = None;
                    self.log_ok(lang.image_ready(
                        &info.name,
                        &format_bytes(info.size),
                    ));
                    self.image_path = info.path.display().to_string();
                    self.image_info = Some(*info);
                }
                Ok(Event::ImageFailed(message)) => {
                    let lang = self.lang();
                    self.loading_image = false;
                    self.image_info = None;
                    self.image_error = Some(message.clone());
                    self.log_error(lang.image_failed_log(&message));
                }
                Ok(Event::Busy(busy)) => {
                    let message = match busy {
                        BusyText::Hashing => self.lang().busy_hashing().to_owned(),
                        BusyText::EnumerateFailed(error) => {
                            self.lang().enumerate_failed(&error)
                        }
                    };
                    self.notice = Some(message);
                }
                Ok(Event::Started(control)) => {
                    self.notice = None;
                    self.log(self.lang().task_accepted());
                    self.control = Some(control);
                    self.stage = Stage::Progress;
                }
                Ok(Event::Finished(result)) => {
                    let lang = self.lang();
                    self.control = None;
                    match result {
                        Ok(outcome) => {
                            let state = lang.burn_state(outcome.state);
                            if outcome.state == BurnState::Completed {
                                self.log_ok(lang.batch_ended(state));
                            } else {
                                self.log_warn(lang.batch_ended(state));
                            }
                            for disk in &outcome.disks {
                                let line = format!(
                                    "  {} · {} · {}",
                                    disk.device,
                                    lang.write_report(&disk.write),
                                    lang.verify_report(&disk.verify)
                                );
                                match &disk.error {
                                    Some(error) => {
                                        self.log_error(line);
                                        self.log_error(lang.reason_line(error));
                                    }
                                    None => self.log(line),
                                }
                            }
                            self.stage = Stage::Done;
                            self.outcome = Some(outcome);
                            self.load_ledger();
                            // 设置里开启「完成后弹出设备」时弹出目标盘。
                            if self.settings.eject_after {
                                let letters = std::mem::take(&mut self.burn_letters);
                                for letter in letters {
                                    if eject_drive(letter) {
                                        self.log_ok(lang.ejected(letter));
                                    } else {
                                        self.log_warn(lang.eject_failed(letter));
                                    }
                                }
                            }
                        }
                        Err(BatchFail::Core(message)) => {
                            self.log_error(lang.batch_failed(&message));
                            self.stage = Stage::Main;
                            self.notice = Some(message);
                        }
                        Err(BatchFail::Ledger(fail)) => {
                            let message = fail.text(lang);
                            self.log_error(lang.batch_failed(&message));
                            self.stage = Stage::Main;
                            self.notice = Some(message);
                        }
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn participating_ids(&self) -> Vec<&DeviceCandidate> {
        self.devices
            .iter()
            .filter(|device| !self.excluded.contains(device.id.as_str()))
            .collect()
    }

    /// C1 / C6：只有参与写入、且状态不是“空盘”的盘才需要 PIN。
    fn needs_pin(&self) -> bool {
        self.participating_ids()
            .iter()
            .any(|device| device.state != DiskState::Empty)
    }

    fn rescan(&mut self) {
        self.scanning = true;
        self.log(self.lang().rescan_log());
        let _ = self.commands.send(Command::Rescan);
    }

    fn inspect_image(&mut self) {
        let lang = self.lang();
        let path = PathBuf::from(self.image_path.trim());
        if path.as_os_str().is_empty() {
            self.image_error = Some(lang.pick_image_first().to_owned());
            return;
        }
        self.image_info = None;
        self.image_error = None;
        self.loading_image = true;
        self.log(lang.read_image_log(&path.display().to_string()));
        let _ = self.commands.send(Command::Inspect(path));
    }

    fn toggle_excluded(&mut self, id: &str) {
        let lang = self.lang();
        if self.excluded.remove(id) {
            self.log(lang.include_log(id));
        } else {
            self.excluded.insert(id.to_owned());
            self.log(lang.exclude_log(id));
        }
        // C6：确认弹窗内排除到不需要 PIN 时，直接开始。
        if self.confirm_open && !self.needs_pin() {
            self.begin_burn();
        }
    }

    fn request_start(&mut self) {
        let lang = self.lang();
        let t = lang.texts();
        if self.image_info.is_none() {
            self.notice = Some(t.reason_no_image.to_owned());
            return;
        }
        if self.participating_ids().is_empty() {
            self.notice = Some(lang.no_targets().to_owned());
            return;
        }
        if self.needs_pin() {
            // C2：每次重新生成 PIN。同时冻结本次确认的设备集合（D4）。
            self.pin_expected = generate_pin();
            self.pin_input.clear();
            self.pin_focus = true;
            self.pending = Some(
                self.devices
                    .iter()
                    .map(|device| device.id.as_str().to_owned())
                    .collect(),
            );
            self.confirm_open = true;
            // 确认期间暂停自动检测，避免列表在弹窗下变动。
            let _ = self.commands.send(Command::SetWatch(false));
            self.log_warn(lang.pin_wait());
        } else {
            self.log(lang.all_empty());
            self.begin_burn();
        }
    }

    fn begin_burn(&mut self) {
        let lang = self.lang();
        let t = lang.texts();
        let Some(info) = self.image_info.as_ref() else {
            self.notice = Some(t.reason_no_image.to_owned());
            return;
        };
        // 只写确认弹窗展示过的设备；确认期间新插入的盘不在集合内（D4）。
        let frozen = self.pending.clone().unwrap_or_else(|| {
            self.devices
                .iter()
                .map(|device| device.id.as_str().to_owned())
                .collect()
        });
        let in_frozen = |device: &DeviceCandidate| frozen.iter().any(|id| id == device.id.as_str());
        let targets: Vec<DeviceId> = self
            .devices
            .iter()
            .filter(|device| in_frozen(device))
            .filter(|device| !self.excluded.contains(device.id.as_str()))
            .map(|device| device.id.clone())
            .collect();
        let excluded: Vec<DeviceId> = self
            .devices
            .iter()
            .filter(|device| in_frozen(device))
            .filter(|device| self.excluded.contains(device.id.as_str()))
            .map(|device| device.id.clone())
            .collect();
        if targets.is_empty() {
            self.notice = Some(lang.no_targets().to_owned());
            return;
        }

        let image_path = info.path.clone();
        self.confirm_open = false;
        self.pending = None;
        let _ = self.commands.send(Command::SetWatch(true));
        self.pin_input.clear();
        self.pin_expected.clear();
        self.outcome = None;
        self.notice = None;
        self.stage = Stage::Progress;
        // 记下目标盘符，供「完成后弹出设备」使用。
        self.burn_letters = self
            .devices
            .iter()
            .filter(|device| targets.iter().any(|id| id == &device.id))
            .flat_map(|device| device.volumes.iter().filter_map(|volume| volume.letter))
            .collect();
        self.log(lang.start_batch(targets.len(), excluded.len()));

        let request = StartRequest {
            image_path,
            targets,
            excluded,
            verify: self.settings.verify,
        };
        let _ = self.commands.send(Command::Start(request));
    }

    fn cancel_burn(&mut self) {
        self.log(self.lang().cancel_requested());
        if let Some(control) = &self.control {
            control.cancel();
        }
        let _ = self.commands.send(Command::Cancel);
    }

    /// 关闭确认弹窗：恢复自动检测，丢弃冻结集合。
    fn cancel_confirm(&mut self, reason: &str) {
        self.log(reason);
        self.confirm_open = false;
        self.pending = None;
        self.pin_input.clear();
        self.pin_expected.clear();
        let _ = self.commands.send(Command::SetWatch(self.auto_watch));
    }

    /// 每 0.5 秒采样一次逐盘速度（MB/s）。
    fn sample_progress(&mut self) {
        let Some(control) = &self.control else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample).as_secs_f64();
        if elapsed < 0.5 {
            return;
        }
        self.last_sample = now;
        for disk in control.disk_progress() {
            let previous = self.prev_bytes.get(&disk.device_id).copied().unwrap_or(0);
            let delta = disk.written_bytes.saturating_sub(previous);
            self.prev_bytes
                .insert(disk.device_id.clone(), disk.written_bytes);
            let speed = delta as f64 / elapsed;
            self.speeds.insert(disk.device_id, speed);
        }
    }

    fn back_to_main(&mut self) {
        self.outcome = None;
        self.stage = Stage::Main;
        self.rescan();
    }

    /// 日志页：级别筛选 chips + 彩色级别点 + 自动滚动（上翻后暂停）。
    fn logs_page(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let t = lang.texts();
        let mut copy = false;
        let mut export = false;
        let mut clear = false;
        let mut open_dir = false;

        ui.horizontal(|ui| {
            ui.label(RichText::new(t.logs_title).size(20.0).strong());
            ui.label(
                RichText::new(lang.n_lines(self.logs.len()))
                    .size(13.5)
                    .color(TEXT_SECOND),
            );
            ui.add_space(12.0);
            for filter in [
                LogFilter::All,
                LogFilter::Info,
                LogFilter::Warn,
                LogFilter::Error,
            ] {
                if filter_chip(
                    ui,
                    filter.label(t),
                    filter.dot_color(),
                    self.log_filter == filter,
                ) {
                    self.log_filter = filter;
                }
            }

            // 右侧：导出 / 复制 / 清空（Ghost）+ 「···」里的打开目录。
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("···", |ui| {
                    if icon_button(ui, Icon::FolderOpen, t.open_log_dir, BtnKind::Ghost, 0.0)
                        .clicked()
                    {
                        open_dir = true;
                        ui.close_menu();
                    }
                });
                if self.log_clear_arm {
                    if ui
                        .add(
                            egui::Button::new(RichText::new(t.confirm_clear).color(Color32::WHITE))
                                .fill(DANGER),
                        )
                        .clicked()
                    {
                        clear = true;
                    }
                    if ui.add(ghost_button(t.cancel, TEXT_SECOND)).clicked() {
                        self.log_clear_arm = false;
                    }
                } else if icon_button(ui, Icon::Delete, t.clear, BtnKind::GhostDanger, 0.0).clicked()
                {
                    self.log_clear_arm = true;
                }
                if icon_button(ui, Icon::Copy, t.copy, BtnKind::Ghost, 0.0).clicked() {
                    copy = true;
                }
                if icon_button(ui, Icon::Download, t.export, BtnKind::Ghost, 0.0).clicked() {
                    export = true;
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.log_query)
                    .hint_text(t.search_logs_hint)
                    .desired_width(220.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut on = self.log_autoscroll;
                if toggle_switch(ui, on) {
                    on = !on;
                }
                if on != self.log_autoscroll {
                    self.log_autoscroll = on;
                }
                ui.label(
                    RichText::new(t.autoscroll)
                        .size(13.0)
                        .color(if self.log_autoscroll {
                            TEXT_SECOND
                        } else {
                            TEXT_HINT
                        }),
                );
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        let filter = self.log_filter;
        let query = self.log_query.trim().to_lowercase();
        let output = egui::ScrollArea::vertical()
            .stick_to_bottom(self.log_autoscroll)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut any = false;
                for line in self.logs.iter().filter(|line| {
                    filter.accepts(line.level)
                        && (query.is_empty() || line.text.to_lowercase().contains(&query))
                }) {
                    any = true;
                    ui.horizontal(|ui| {
                        // 级别色点。
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 20.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, log_color(line.level));
                        ui.add_sized(
                            [78.0, 20.0],
                            egui::Label::new(
                                RichText::new(&line.time).monospace().size(13.0).color(TEXT_HINT),
                            ),
                        );
                        ui.label(
                            RichText::new(&line.text)
                                .size(14.0)
                                .color(if line.level == LogLevel::Error {
                                    DANGER
                                } else {
                                    Color32::from_rgb(0xB8, 0xBE, 0xC9)
                                }),
                        );
                    });
                }
                if !any {
                    ui.add_space(24.0);
                    ui.label(
                        RichText::new(if query.is_empty() {
                            t.empty_level
                        } else {
                            t.no_match_logs
                        })
                        .size(14.0)
                        .color(TEXT_HINT),
                    );
                }
            });

        // 用户上翻后暂停自动滚动。
        let at_bottom = output.state.offset.y + output.inner_rect.height()
            >= output.content_size.y - 8.0;
        if !at_bottom {
            self.log_autoscroll = false;
        }

        if copy {
            ui.ctx().copy_text(self.logs_as_text());
        }
        if export {
            let text = self.logs_as_text();
            self.export_logs(text);
        }
        if clear {
            if let Some(path) = self.log_file.clone() {
                let _ = std::fs::File::create(path);
            }
            self.logs.clear();
            self.log_clear_arm = false;
            self.log_autoscroll = true;
            self.log_ok(t.logs_cleared);
        }
        if open_dir && !open_logs_dir() {
            self.log_error(t.logs_dir_failed);
        }
    }

    /// 读取台账（ledger.jsonl）：`batch` 起一条记录，随后的 `disk` 归到该记录。
    fn load_ledger(&mut self) {
        self.ledger.clear();
        let Ok(path) = ledger_path() else {
            return;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            return;
        };
        for raw in content.lines() {
            let raw = raw.trim_start_matches('\u{feff}');
            if raw.trim().is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
                continue;
            };
            match value.get("kind").and_then(|kind| kind.as_str()) {
                Some("batch") => {
                    let at = text_of(&value, "at");
                    self.ledger.push(LedgerRecord {
                        started: chrono::DateTime::parse_from_rfc3339(&at).ok(),
                        ended: None,
                        at,
                        image_name: text_of(&value, "image_name"),
                        image_path: text_of(&value, "image_path"),
                        image_size: value
                            .get("image_size")
                            .and_then(|item| item.as_u64())
                            .unwrap_or(0),
                        total: value
                            .get("targets")
                            .and_then(|targets| targets.as_array())
                            .map(|items| items.len())
                            .unwrap_or(0),
                        succeeded: 0,
                        excluded: value
                            .get("excluded")
                            .and_then(|items| items.as_array())
                            .map(|items| items.len())
                            .unwrap_or(0),
                        disks: Vec::new(),
                        raw: vec![raw.to_string()],
                    });
                }
                Some("disk") => {
                    let lang = self.lang();
                    let Some(record) = self.ledger.last_mut() else {
                        continue;
                    };
                    let write = lang.describe_status(value.get("write"), false);
                    let verify = lang.describe_status(value.get("verify"), true);
                    let ok = write.1 && verify.1;
                    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(&text_of(&value, "at"))
                    {
                        if record.ended.map_or(true, |end| time > end) {
                            record.ended = Some(time);
                        }
                    }
                    record.raw.push(raw.to_string());
                    record.disks.push(DiskRecord {
                        device: text_of(&value, "device_id"),
                        model: text_of(&value, "device_model"),
                        letters: text_of(&value, "letters"),
                        capacity: value
                            .get("capacity_bytes")
                            .and_then(|item| item.as_u64())
                            .unwrap_or(0),
                        write: write.0,
                        verify: verify.0,
                        error: report_reason(value.get("write"))
                            .or_else(|| report_reason(value.get("verify"))),
                        ok,
                    });
                    if ok {
                        record.succeeded += 1;
                    }
                }
                _ => {}
            }
        }
        // 最新的排在最前。
        self.ledger.reverse();
        // 刷新后清理已经不存在的选择项。
        let alive: HashSet<String> = self.ledger.iter().map(|record| record.key()).collect();
        self.ledger_selected.retain(|key| alive.contains(key));
        if let Some(open) = &self.ledger_open {
            if !alive.contains(open) {
                self.ledger_open = None;
            }
        }
    }

    /// 按原始行回写台账（删除单条 / 批量删除 / 清空）。
    fn rewrite_ledger(&self) {
        let Ok(path) = ledger_path() else {
            return;
        };
        let mut out = String::new();
        for record in self.ledger.iter().rev() {
            for line in &record.raw {
                out.push_str(line);
                out.push('\n');
            }
        }
        let _ = std::fs::write(path, out);
    }

    /// 删除指定 key 的记录。
    fn delete_records(&mut self, keys: &HashSet<String>) {
        let before = self.ledger.len();
        self.ledger.retain(|record| !keys.contains(&record.key()));
        let removed = before - self.ledger.len();
        if removed == 0 {
            return;
        }
        self.rewrite_ledger();
        self.ledger_selected.retain(|key| !keys.contains(key));
        if let Some(open) = &self.ledger_open {
            if keys.contains(open) {
                self.ledger_open = None;
            }
        }
        let message = self.lang().records_deleted(removed);
        self.log_ok(message);
    }

    fn save_settings(&self) {
        let Some(path) = settings_path() else {
            return;
        };
        if let Ok(text) = serde_json::to_string_pretty(&self.settings) {
            let _ = std::fs::write(path, text);
        }
    }

    /// 左侧导航：当前页高亮（底色 + 3px 主色指示条）。
    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .exact_width(152.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(0x11, 0x13, 0x17))
                    .inner_margin(egui::Margin::symmetric(8.0, 12.0)),
            )
            .show(ctx, |ui| {
                let t = self.lang().texts();
                // 品牌：主标题取当前语言的品牌名，英文名作副标题。
                ui.horizontal(|ui| {
                    ui.add_space(6.0);
                    draw_logo(ui, &self.logo, 28.0);
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(t.brand).size(19.0).strong().color(TEXT_MAIN));
                        ui.label(
                            RichText::new("Qike Flash")
                                .size(12.0)
                                .color(TEXT_SECOND),
                        );
                    });
                });
                ui.add_space(16.0);
                let pages = [
                    Page::Home,
                    Page::Logs,
                    Page::History,
                    Page::Settings,
                    Page::About,
                ];
                for page in pages {
                    let selected = self.page == page;
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 36.0),
                        egui::Sense::click(),
                    );
                    // 选中底 / 悬停底 / 左侧指示条都做 150ms 过渡。
                    let sel_t = anim_bool(ui, response.id.with("sel"), selected, 0.15);
                    let hov_t = anim_bool(ui, response.id.with("hov"), response.hovered(), 0.12);
                    let painter = ui.painter();
                    let fill = mix_color(
                        mix_color(Color32::TRANSPARENT, Color32::from_white_alpha(8), hov_t),
                        CARD_HOVER,
                        sel_t,
                    );
                    if fill.a() > 0 {
                        painter.rect_filled(rect, 8.0, fill);
                    }
                    // 左侧 3px 主色指示条：高度随选中进度从中间展开。
                    if sel_t > 0.0 {
                        let full = rect.height() - 12.0;
                        let bar = egui::Rect::from_center_size(
                            egui::pos2(rect.left() + 1.5, rect.center().y),
                            egui::vec2(3.0, full * sel_t),
                        );
                        painter.rect_filled(bar, 2.0, PRIMARY);
                    }
                    let color = mix_color(TEXT_SECOND, TEXT_MAIN, sel_t);
                    let icon_rect = egui::Rect::from_center_size(
                        egui::pos2(rect.left() + 20.0, rect.center().y),
                        egui::vec2(16.0, 16.0),
                    );
                    icons::paint(painter, icon_rect, page.icon(), color);
                    painter.text(
                        egui::pos2(rect.left() + 36.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        page.label(t),
                        egui::FontId::proportional(15.0),
                        color,
                    );
                    if response.clicked() {
                        self.page = page;
                        if page == Page::History {
                            self.load_ledger();
                        }
                    }
                }
            });
    }

    /// 历史页：台账记录（状态 → 文件名 → 指标），支持筛选、分组、多选与导出。
    fn history_page(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let t = lang.texts();
        let mut reload = false;
        let mut clear = false;
        let mut action: Option<LedgerAction> = None;
        let mut pending_delete: Option<HashSet<String>> = None;
        let mut export_request: Option<(ExportKind, Option<HashSet<String>>)> = None;

        ui.horizontal(|ui| {
            ui.label(RichText::new(t.history_title).size(20.0).strong());
            ui.label(
                RichText::new(lang.n_writes(self.ledger.len()))
                    .size(13.5)
                    .color(TEXT_SECOND),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // 导出与清空收进「⋯」，破坏性操作不占主视线。
                ui.menu_button("···", |ui| {
                    if icon_button(ui, Icon::Download, t.export_csv, BtnKind::Ghost, 0.0).clicked()
                    {
                        export_request = Some((ExportKind::Csv, None));
                        ui.close_menu();
                    }
                    if icon_button(ui, Icon::Download, t.export_json, BtnKind::Ghost, 0.0).clicked()
                    {
                        export_request = Some((ExportKind::Json, None));
                        ui.close_menu();
                    }
                    ui.separator();
                    if icon_button(ui, Icon::Delete, t.clear_records, BtnKind::GhostDanger, 0.0)
                        .clicked()
                    {
                        self.ledger_clear_arm = true;
                        ui.close_menu();
                    }
                });
                if icon_button(ui, Icon::Refresh, t.refresh, BtnKind::Ghost, 0.0).clicked() {
                    reload = true;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.ledger_query)
                        .hint_text(t.ledger_search_hint)
                        .desired_width(220.0),
                );
            });
        });

        // 清空确认：二次确认，并询问是否同时清空日志。
        if self.ledger_clear_arm {
            ui.add_space(8.0);
            egui::Frame::none()
                .fill(Color32::from_rgb(0x2E, 0x1D, 0x21))
                .rounding(8.0)
                .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t.clear_records)
                                .size(13.5)
                                .strong()
                                .color(DANGER),
                        );
                        let mut also_logs = self.ledger_clear_logs;
                        if ui
                            .add(egui::Checkbox::new(&mut also_logs, t.clear_logs_too))
                            .changed()
                        {
                            self.ledger_clear_logs = also_logs;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(t.confirm_clear).color(Color32::WHITE),
                                    )
                                    .fill(DANGER),
                                )
                                .clicked()
                            {
                                clear = true;
                            }
                            if ui.add(ghost_button(t.cancel, TEXT_SECOND)).clicked() {
                                self.ledger_clear_arm = false;
                            }
                        });
                    });
                });
        }

        // 统计条：成功 / 失败批次与总写入量，右侧放状态筛选。
        ui.add_space(10.0);
        let ok_batches = self.ledger.iter().filter(|record| record.all_ok()).count();
        let failed_batches = self.ledger.len() - ok_batches;
        let written: u64 = self
            .ledger
            .iter()
            .map(|record| record.image_size.saturating_mul(record.succeeded as u64))
            .sum();
        egui::Frame::none()
            .fill(CARD)
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(16.0, 10.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    stat_block(ui, t.stat_ok, &ok_batches.to_string(), SUCCESS);
                    ui.add_space(24.0);
                    stat_block(
                        ui,
                        t.stat_failed,
                        &failed_batches.to_string(),
                        if failed_batches > 0 { DANGER } else { TEXT_SECOND },
                    );
                    ui.add_space(24.0);
                    stat_block(ui, t.stat_written, &format_bytes(written), TEXT_MAIN);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for filter in [LedgerFilter::Failed, LedgerFilter::Ok, LedgerFilter::All] {
                            let dot = match filter {
                                LedgerFilter::All => TEXT_SECOND,
                                LedgerFilter::Ok => SUCCESS,
                                LedgerFilter::Failed => DANGER,
                            };
                            if filter_chip(ui, filter.label(t), dot, self.ledger_filter == filter) {
                                self.ledger_filter = filter;
                            }
                        }
                    });
                });
            });

        // 多选操作条：导出所选 / 删除所选（二次确认）。
        if !self.ledger_selected.is_empty() {
            let count = self.ledger_selected.len();
            ui.add_space(8.0);
            egui::Frame::none()
                .fill(CARD_HOVER)
                .rounding(8.0)
                .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(lang.selected_n(count)).size(13.0).strong());
                        if ui
                            .add(ghost_button(t.cancel_selection, TEXT_SECOND))
                            .clicked()
                        {
                            self.ledger_selected.clear();
                            self.ledger_delete_arm = false;
                        }
                        ui.menu_button(lang.export_selected(count), |ui| {
                            if ui.button(t.export_csv).clicked() {
                                export_request =
                                    Some((ExportKind::Csv, Some(self.ledger_selected.clone())));
                                ui.close_menu();
                            }
                            if ui.button(t.export_json).clicked() {
                                export_request =
                                    Some((ExportKind::Json, Some(self.ledger_selected.clone())));
                                ui.close_menu();
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if self.ledger_delete_arm {
                                format!("{}?", lang.delete_selected(count))
                            } else {
                                lang.delete_selected(count)
                            };
                            let (fill, color) = if self.ledger_delete_arm {
                                (DANGER, Color32::WHITE)
                            } else {
                                (Color32::TRANSPARENT, DANGER)
                            };
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(label).color(color))
                                        .fill(fill)
                                        .stroke(if self.ledger_delete_arm {
                                            egui::Stroke::NONE
                                        } else {
                                            egui::Stroke::new(1.0_f32, DANGER)
                                        }),
                                )
                                .clicked()
                            {
                                if self.ledger_delete_arm {
                                    pending_delete = Some(self.ledger_selected.clone());
                                    self.ledger_delete_arm = false;
                                } else {
                                    self.ledger_delete_arm = true;
                                }
                            }
                        });
                    });
                });
        }

        ui.add_space(10.0);

        let filter = self.ledger_filter;
        let query = self.ledger_query.trim().to_lowercase();
        let open = self.ledger_open.clone();
        let selected = self.ledger_selected.clone();

        if self.ledger.is_empty() {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(t.empty_ledger_title)
                        .size(15.0)
                        .color(TEXT_SECOND),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(t.empty_ledger_hint)
                        .size(13.0)
                        .color(TEXT_HINT),
                );
            });
        } else {
            // 按日期分组，最新的排在最前；写入进行中由 update() 自动刷新。
            let ledger = &self.ledger;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut group = String::new();
                    let mut shown = 0usize;
                    for record in ledger {
                        if !filter.matches(record) {
                            continue;
                        }
                        if !query.is_empty() && !record_matches(record, &query) {
                            continue;
                        }
                        let day = day_label(&record.at, lang);
                        if day != group {
                            if shown > 0 {
                                ui.add_space(8.0);
                            }
                            ui.label(
                                RichText::new(&day)
                                    .size(12.5)
                                    .strong()
                                    .color(TEXT_HINT),
                            );
                            ui.add_space(4.0);
                            group = day;
                        }
                        shown += 1;
                        let key = record.key();
                        let expanded = open.as_ref().map_or(false, |open| open == &key);
                        let is_selected = selected.contains(&key);
                        if let Some(next) = Self::ledger_row(ui, record, expanded, is_selected, lang)
                        {
                            action = Some(next);
                        }
                        ui.add_space(8.0);
                    }
                    if shown == 0 {
                        ui.add_space(24.0);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(t.no_match_records)
                                    .size(13.5)
                                    .color(TEXT_HINT),
                            );
                        });
                    }
                });
        }

        if let Some(keys) = pending_delete {
            self.delete_records(&keys);
        }
        if let Some(next) = action.take() {
            self.apply_ledger_action(next);
        }
        if reload {
            self.load_ledger();
        }
        if clear {
            if let Ok(path) = ledger_path() {
                let _ = std::fs::File::create(path);
            }
            self.ledger.clear();
            self.ledger_selected.clear();
            self.ledger_open = None;
            self.ledger_clear_arm = false;
            if self.ledger_clear_logs {
                if let Some(path) = self.log_file.clone() {
                    let _ = std::fs::File::create(path);
                }
                self.logs.clear();
            }
            let message = t.ledger_cleared;
            self.log_ok(message);
        }
        if let Some((kind, keys)) = export_request {
            self.export_ledger(kind, keys.as_ref());
        }
    }

    /// 历史页行内动作：遍历结束后统一应用，避免与记录列表的借用冲突。
    fn apply_ledger_action(&mut self, action: LedgerAction) {
        match action {
            LedgerAction::Toggle(key) => {
                self.ledger_open = if self.ledger_open.as_deref() == Some(key.as_str()) {
                    None
                } else {
                    Some(key)
                };
            }
            LedgerAction::Select(key) => {
                if !self.ledger_selected.remove(&key) {
                    self.ledger_selected.insert(key);
                }
                self.ledger_delete_arm = false;
            }
            LedgerAction::Delete(key) => {
                let keys: HashSet<String> = [key].into_iter().collect();
                self.delete_records(&keys);
            }
            LedgerAction::Retry(key) => {
                let target = self
                    .ledger
                    .iter()
                    .find(|record| record.key() == key)
                    .map(|record| (record.image_path.clone(), record.image_name.clone()));
                let lang = self.lang();
                match target {
                    Some((path, name)) if !path.is_empty() && PathBuf::from(&path).exists() => {
                        self.page = Page::Home;
                        self.stage = Stage::Main;
                        let message = lang.retry_prefilled(&name);
                        self.log(message);
                        let _ = self.commands.send(Command::Inspect(PathBuf::from(path)));
                    }
                    _ => self.log_warn(lang.texts().retry_missing_image),
                }
            }
            LedgerAction::ViewLog(key) => {
                let name = self
                    .ledger
                    .iter()
                    .find(|record| record.key() == key)
                    .map(|record| record.image_name.clone());
                if let Some(name) = name {
                    self.log_query = name;
                    self.log_filter = LogFilter::All;
                    self.log_autoscroll = false;
                    self.page = Page::Logs;
                }
            }
        }
    }

    /// 导出历史记录：CSV 每盘一行，JSON 带逐盘错误详情。
    fn export_ledger(&mut self, kind: ExportKind, keys: Option<&HashSet<String>>) {
        let lang = self.lang();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let default = match kind {
            ExportKind::Csv => format!("qike-history-{stamp}.csv"),
            ExportKind::Json => format!("qike-history-{stamp}.json"),
        };
        let Some(path) = pick_save_file(&default) else {
            return;
        };
        let records: Vec<&LedgerRecord> = self
            .ledger
            .iter()
            .filter(|record| keys.map_or(true, |keys| keys.contains(&record.key())))
            .collect();

        let text = match kind {
            ExportKind::Csv => {
                let mut out = String::from(
                    "time,image,image_path,image_size,total,succeeded,drive,model,device,capacity,write,verify,error\n",
                );
                for record in &records {
                    if record.disks.is_empty() {
                        let fields = [
                            full_time(&record.at),
                            record.image_name.clone(),
                            record.image_path.clone(),
                            record.image_size.to_string(),
                            record.total.to_string(),
                            record.succeeded.to_string(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                        ];
                        out.push_str(&fields.map(|field| csv_field(&field)).join(","));
                        out.push('\n');
                        continue;
                    }
                    for disk in &record.disks {
                        let fields = [
                            full_time(&record.at),
                            record.image_name.clone(),
                            record.image_path.clone(),
                            record.image_size.to_string(),
                            record.total.to_string(),
                            record.succeeded.to_string(),
                            disk.letters.clone(),
                            disk.model.clone(),
                            disk.device.clone(),
                            disk.capacity.to_string(),
                            disk.write.clone(),
                            disk.verify.clone(),
                            disk.error.clone().unwrap_or_default(),
                        ];
                        out.push_str(&fields.map(|field| csv_field(&field)).join(","));
                        out.push('\n');
                    }
                }
                out
            }
            ExportKind::Json => {
                let items: Vec<serde_json::Value> = records
                    .iter()
                    .map(|record| {
                        let disks: Vec<serde_json::Value> = record
                            .disks
                            .iter()
                            .map(|disk| {
                                serde_json::json!({
                                    "device": disk.device,
                                    "model": disk.model,
                                    "letters": disk.letters,
                                    "capacity": disk.capacity,
                                    "write": disk.write,
                                    "verify": disk.verify,
                                    "ok": disk.ok,
                                    "error": disk.error,
                                })
                            })
                            .collect();
                        serde_json::json!({
                            "time": record.at,
                            "image": record.image_name,
                            "image_path": record.image_path,
                            "image_size": record.image_size,
                            "total": record.total,
                            "succeeded": record.succeeded,
                            "excluded": record.excluded,
                            "disks": disks,
                        })
                    })
                    .collect();
                serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_owned())
            }
        };

        match std::fs::write(&path, text) {
            Ok(()) => self.log_ok(lang.ledger_exported(&path)),
            Err(error) => self.log_error(lang.ledger_export_failed(&error.to_string())),
        }
    }

    /// 单条历史记录：第一行 状态 → 文件名 → 时间/重试/展开，第二行指标，失败时附错误摘要。
    fn ledger_row(
        ui: &mut egui::Ui,
        record: &LedgerRecord,
        expanded: bool,
        selected: bool,
        lang: Lang,
    ) -> Option<LedgerAction> {
        let t = lang.texts();
        let key = record.key();
        let mut action: Option<LedgerAction> = None;
        // 整行作为点击 / 右键容器：scope 的交互先于子控件注册，子按钮优先响应，
        // 避免像 `Response::interact` 那样在末尾补注册而抢掉行内按钮的点击。
        let row = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            Self::ledger_row_content(ui, record, expanded, selected, lang, &mut action);
        });
        let row = row.response;

        // 行 hover：120ms 淡入提亮，不改变布局。
        let hover_t = anim_bool(ui, row.id.with("hover"), row.hovered(), 0.12);
        if hover_t > 0.0 {
            ui.painter().rect_filled(
                row.rect,
                8.0,
                Color32::from_rgba_premultiplied(
                    (255.0 * 0.024 * hover_t) as u8,
                    (255.0 * 0.024 * hover_t) as u8,
                    (255.0 * 0.024 * hover_t) as u8,
                    (6.0 * hover_t) as u8,
                ),
            );
        }
        // 点击行任意空白处展开 / 收起详情。
        if row.clicked() {
            action = Some(LedgerAction::Toggle(key.clone()));
        }

        // 右键菜单：复制镜像名 / 错误信息，重试，删除此条。
        row.context_menu(|ui| {
            if ui.button(t.copy_image_name).clicked() {
                ui.ctx().copy_text(record.image_name.clone());
                ui.close_menu();
            }
            if let Some(error) = record.first_error() {
                if ui.button(t.copy_error_text).clicked() {
                    ui.ctx().copy_text(error.to_owned());
                    ui.close_menu();
                }
            }
            if ui.button(t.retry).clicked() {
                action = Some(LedgerAction::Retry(key.clone()));
                ui.close_menu();
            }
            if ui.button(t.delete_record).clicked() {
                action = Some(LedgerAction::Delete(key.clone()));
                ui.close_menu();
            }
        });

        action
    }

    /// 历史记录行的内容：徽章、指标、错误摘要与展开后的逐盘结果。
    fn ledger_row_content(
        ui: &mut egui::Ui,
        record: &LedgerRecord,
        expanded: bool,
        selected: bool,
        lang: Lang,
        action: &mut Option<LedgerAction>,
    ) {
        let t = lang.texts();
        let key = record.key();
        egui::Frame::none()
            .fill(CARD)
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(12.0, 10.0))
            .show(ui, |ui| {
                // 第一行：状态徽章在文件名左侧，视线从左往右扫。
                ui.horizontal(|ui| {
                    let mut checked = selected;
                    if ui.add(egui::Checkbox::new(&mut checked, "")).changed() {
                        *action = Some(LedgerAction::Select(key.clone()));
                    }
                    let (badge, ok) = if record.all_ok() {
                        (lang.batch_ok(record.total), true)
                    } else {
                        let separator = if lang == Lang::En { ", " } else { "、" };
                        let mut failed = record.failed_labels();
                        if failed.is_empty() {
                            failed.push((record.total - record.succeeded).to_string());
                        }
                        (
                            lang.batch_partial(record.succeeded, record.total, &failed.join(separator)),
                            false,
                        )
                    };
                    let _ = status_badge(ui, ok, &badge);
                    let name = ui.label(
                        RichText::new(truncate_chars(&record.image_name, 40))
                            .size(14.5)
                            .strong(),
                    );
                    if !record.image_path.is_empty() {
                        name.on_hover_text(&record.image_path);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_only_button(
                            ui,
                            if expanded {
                                Icon::ChevronDown
                            } else {
                                Icon::ChevronRight
                            },
                            TEXT_SECOND,
                        )
                        .clicked()
                        {
                            *action = Some(LedgerAction::Toggle(key.clone()));
                        }
                        if !ok && icon_button(ui, Icon::Refresh, t.retry, BtnKind::Ghost, 0.0).clicked()
                        {
                            *action = Some(LedgerAction::Retry(key.clone()));
                        }
                        let time = ui.label(
                            RichText::new(relative_time(&record.at, lang))
                                .size(12.5)
                                .color(TEXT_HINT),
                        );
                        time.on_hover_text(full_time(&record.at));
                    });
                });

                // 第二行：盘符 / 型号 / 容量 / 均速 / 用时；整行点击由外层 scope 处理。
                ui.horizontal(|ui| {
                        let letters = batch_letters(record);
                        if !letters.is_empty() {
                            ui.label(
                                RichText::new(letters)
                                    .monospace()
                                    .size(12.5)
                                    .color(TEXT_MAIN),
                            );
                        }
                        let model = batch_model(record);
                        if !model.is_empty() {
                            let label = ui.label(
                                RichText::new(truncate_chars(model, 26))
                                    .size(12.5)
                                    .color(TEXT_SECOND),
                            );
                            if model.chars().count() > 26 {
                                label.on_hover_text(model);
                            }
                        }
                        let capacity = batch_capacity(record);
                        if capacity > 0 {
                            ui.label(
                                RichText::new(format_bytes(capacity))
                                    .size(12.5)
                                    .color(TEXT_HINT),
                            );
                        }
                        if let Some(speed) = batch_speed(record) {
                            ui.label(
                                RichText::new(lang.avg_speed(&speed))
                                    .size(12.5)
                                    .color(TEXT_HINT),
                            );
                        }
                        if let Some(duration) = lang.format_duration(record.started, record.ended) {
                            ui.label(
                                RichText::new(lang.elapsed(&duration))
                                    .size(12.5)
                                    .color(TEXT_HINT),
                            );
                        }
                        if record.excluded > 0 {
                            ui.label(
                                RichText::new(lang.skipped_n(record.excluded))
                                    .size(12.5)
                                    .color(TEXT_HINT),
                            )
                            .on_hover_text(t.skipped_tip);
                        }
                    });

                // 失败时行内给一句错误摘要，并可直接跳到日志页定位。
                if let Some(error) = record.first_error() {
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(truncate_chars(error, 96))
                                .size(12.5)
                                .color(DANGER),
                        )
                        .on_hover_text(error);
                        if ui
                            .add(ghost_button(
                                format!("{} →", t.view_full_log),
                                TEXT_SECOND,
                            ))
                            .clicked()
                        {
                            *action = Some(LedgerAction::ViewLog(key.clone()));
                        }
                    });
                }

                // 展开：逐盘结果与错误详情（150ms 淡入）。
                let body_t = anim_bool(ui, ui.id().with(&key).with("body"), expanded, 0.15);
                if expanded {
                    ui.set_opacity(body_t);
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);
                    if record.disks.is_empty() {
                        ui.label(
                            RichText::new(t.no_disk_records)
                                .size(13.0)
                                .color(TEXT_HINT),
                        );
                    }
                    for disk in &record.disks {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(14.0, 20.0), egui::Sense::hover());
                            icons::paint(
                                ui.painter(),
                                egui::Rect::from_center_size(
                                    rect.center(),
                                    egui::vec2(12.0, 12.0),
                                ),
                                if disk.ok { Icon::Check } else { Icon::Close },
                                if disk.ok { SUCCESS } else { DANGER },
                            );
                            if !disk.model.is_empty() {
                                let label = ui.label(
                                    RichText::new(truncate_chars(&disk.model, 30))
                                        .size(13.0)
                                        .color(TEXT_MAIN),
                                );
                                if disk.model.chars().count() > 30 {
                                    label.on_hover_text(&disk.model);
                                }
                            }
                            if !disk.letters.is_empty() {
                                ui.label(
                                    RichText::new(&disk.letters)
                                        .monospace()
                                        .size(13.0)
                                        .color(TEXT_MAIN),
                                );
                            }
                            if disk.capacity > 0 {
                                ui.label(
                                    RichText::new(format_bytes(disk.capacity))
                                        .size(12.5)
                                        .color(TEXT_SECOND),
                                );
                            }
                            let id = ui.label(
                                RichText::new(short_device(&disk.device))
                                    .monospace()
                                    .size(12.0)
                                    .color(TEXT_HINT),
                            );
                            id.on_hover_text(&disk.device);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("{} · {}", disk.write, disk.verify))
                                            .size(12.5)
                                            .color(TEXT_HINT),
                                    );
                                },
                            );
                        });
                        if let Some(error) = &disk.error {
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);
                                ui.label(
                                    RichText::new(truncate_chars(error, 120))
                                        .size(12.0)
                                        .color(DANGER),
                                )
                                .on_hover_text(error);
                            });
                        }
                    }
                    ui.set_opacity(1.0);
                }
            });
    }

    /// 设置页：按 通用 / 设备 / 写入 / 日志 分组，改动立即保存并生效。
    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang();
        let t = lang.texts();
        ui.label(RichText::new(t.settings_title).size(20.0).strong());
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let mut changed = false;

        group_label(ui, t.group_general);
        let mut current = self.settings.lang;
        setting_row(ui, t.language, t.language_desc, |ui| {
            for (option, label) in [
                (Lang::ZhCn, t.lang_zh_cn),
                (Lang::ZhTw, t.lang_zh_tw),
                (Lang::En, t.lang_en),
            ] {
                let selected = current == option;
                let (fill, color) = if selected {
                    (PRIMARY, Color32::WHITE)
                } else {
                    (Color32::TRANSPARENT, TEXT_SECOND)
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(label).color(color))
                            .fill(fill)
                            .stroke(if selected {
                                egui::Stroke::NONE
                            } else {
                                egui::Stroke::new(1.0_f32, BORDER)
                            })
                            .min_size(egui::vec2(86.0, 26.0)),
                    )
                    .clicked()
                {
                    current = option;
                }
            }
        });
        if current != self.settings.lang {
            self.settings.lang = current;
            changed = true;
        }
        ui.add_space(16.0);

        group_label(ui, t.group_device);
        let mut auto = self.settings.auto_detect;
        setting_row(ui, t.auto_detect, t.auto_detect_desc, |ui| {
            if toggle_switch(ui, auto) {
                auto = !auto;
            }
        });
        if auto != self.settings.auto_detect {
            self.settings.auto_detect = auto;
            changed = true;
        }
        ui.add_space(16.0);

        group_label(ui, t.group_write);
        let mut verify = self.settings.verify;
        setting_row(ui, t.verify_after, t.verify_after_desc, |ui| {
            if toggle_switch(ui, verify) {
                verify = !verify;
            }
        });
        if verify != self.settings.verify {
            self.settings.verify = verify;
            changed = true;
        }
        ui.add_space(8.0);

        let mut eject = self.settings.eject_after;
        setting_row(ui, t.eject_after, t.eject_after_desc, |ui| {
            if toggle_switch(ui, eject) {
                eject = !eject;
            }
        });
        if eject != self.settings.eject_after {
            self.settings.eject_after = eject;
            changed = true;
        }
        ui.add_space(16.0);

        group_label(ui, t.group_log);
        let mut keep = self.settings.log_keep_days;
        setting_row(ui, t.keep_days, t.keep_days_desc, |ui| {
            // right_to_left 布局：倒序迭代才能按 7 / 30 / 90 / 永久 从左到右显示。
            for days in [0u32, 90, 30, 7] {
                let selected = keep == days;
                let label = if days == 0 {
                    t.forever.to_owned()
                } else {
                    lang.days(days)
                };
                let (fill, color) = if selected {
                    (PRIMARY, Color32::WHITE)
                } else {
                    (Color32::TRANSPARENT, TEXT_SECOND)
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(label).color(color))
                            .fill(fill)
                            .stroke(if selected {
                                egui::Stroke::NONE
                            } else {
                                egui::Stroke::new(1.0_f32, BORDER)
                            })
                            .min_size(egui::vec2(58.0, 26.0)),
                    )
                    .clicked()
                {
                    keep = days;
                }
            }
        });
        if keep != self.settings.log_keep_days {
            self.settings.log_keep_days = keep;
            changed = true;
        }

        if changed {
            self.auto_watch = self.settings.auto_detect;
            let _ = self.commands.send(Command::SetWatch(self.settings.auto_detect));
            self.save_settings();
            self.log(self.lang().texts().settings_updated);
        }
    }

    /// 导出日志到文件：原生「另存为」对话框，结果写入实时日志。
    fn export_logs(&mut self, content: String) {
        let lang = self.lang();
        let default = format!(
            "multi-burn-logs-{}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        let Some(path) = pick_save_file(&default) else {
            return;
        };
        match std::fs::write(&path, &content) {
            Ok(()) => self.log_ok(lang.logs_exported(&path)),
            Err(error) => self.log_error(lang.logs_export_failed(&error.to_string())),
        }
    }
}

/// 关于页列出的一项开源组件。
struct Component {
    name: &'static str,
    license: &'static str,
    /// 用途条目（文案随语言切换）。
    usage: Usage,
    url: &'static str,
}

/// 直接依赖与界面图标来源。
const OPEN_SOURCE: &[Component] = &[
    Component {
        name: "思源黑体 Source Han Sans",
        license: "SIL OFL 1.1",
        usage: Usage::Font,
        url: "https://github.com/adobe-fonts/source-han-sans",
    },
    Component {
        name: "eframe / egui",
        license: "MIT OR Apache-2.0",
        usage: Usage::Ui,
        url: "https://github.com/emilk/egui",
    },
    Component {
        name: "winit",
        license: "Apache-2.0",
        usage: Usage::Window,
        url: "https://github.com/rust-windowing/winit",
    },
    Component {
        name: "serde / serde_json",
        license: "MIT OR Apache-2.0",
        usage: Usage::Storage,
        url: "https://github.com/serde-rs/serde",
    },
    Component {
        name: "sha2",
        license: "MIT OR Apache-2.0",
        usage: Usage::Sha,
        url: "https://github.com/RustCrypto/hashes",
    },
    Component {
        name: "xz2",
        license: "MIT OR Apache-2.0",
        usage: Usage::Xz,
        url: "https://github.com/alexcrichton/xz2-rs",
    },
    Component {
        name: "chrono",
        license: "MIT OR Apache-2.0",
        usage: Usage::Time,
        url: "https://github.com/chronotope/chrono",
    },
    Component {
        name: "windows-rs",
        license: "MIT OR Apache-2.0",
        usage: Usage::Win32,
        url: "https://github.com/microsoft/windows-rs",
    },
    Component {
        name: "IconPark",
        license: "Apache-2.0",
        usage: Usage::Icon,
        url: "https://github.com/bytedance/iconpark",
    },
];

/// 关于页：版本、作者与开源软件，点击条目打开项目主页。
fn about_page(ui: &mut egui::Ui, logo: &egui::TextureHandle, lang: Lang) {
    let t = lang.texts();
    ui.horizontal(|ui| {
        ui.label(RichText::new(t.about_title).size(20.0).strong());
        ui.label(RichText::new("Qike Flash").size(13.5).color(TEXT_SECOND));
    });
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(
        RichText::new(t.about_intro)
            .size(13.0)
            .color(TEXT_SECOND),
    );
    ui.add_space(16.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::none()
                .fill(CARD)
                .rounding(8.0)
                .inner_margin(egui::Margin::symmetric(16.0, 14.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        draw_logo(ui, logo, 56.0);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("齐刻 Qike Flash").size(20.0).strong());
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(lang.version_line(APP_VERSION))
                                    .size(13.0)
                                    .color(TEXT_SECOND),
                            );
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(t.author).size(13.0).color(TEXT_HINT));
                                ui.add(egui::Hyperlink::from_label_and_url(
                                    RichText::new("youye-luna").size(13.0),
                                    "https://github.com/youye-luna",
                                ));
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(t.repo).size(13.0).color(TEXT_HINT));
                                ui.add(egui::Hyperlink::from_label_and_url(
                                    RichText::new("github.com/youye-luna/qike-flash").size(13.0),
                                    "https://github.com/youye-luna/qike-flash",
                                ));
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(t.license_label).size(13.0).color(TEXT_HINT));
                                ui.add(egui::Hyperlink::from_label_and_url(
                                    RichText::new("Apache-2.0").size(13.0),
                                    "https://www.apache.org/licenses/LICENSE-2.0",
                                ));
                            });
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .add(secondary_button(
                                        t.copy_version,
                                        egui::vec2(112.0, 28.0),
                                    ))
                                    .on_hover_text(t.copy_version_tip)
                                    .clicked()
                                {
                                    ui.ctx().copy_text(lang.issue_copy_text(APP_VERSION));
                                }
                            },
                        );
                    });
                });

            ui.add_space(16.0);
            ui.label(RichText::new(t.open_source).size(15.0).strong());
            ui.add_space(2.0);
            ui.label(
                RichText::new(t.open_source_hint)
                    .size(12.5)
                    .color(TEXT_HINT),
            );
            ui.add_space(6.0);
            egui::Frame::none()
                .fill(CARD)
                .rounding(8.0)
                .inner_margin(egui::Margin::symmetric(6.0, 6.0))
                .show(ui, |ui| {
                    for item in OPEN_SOURCE {
                        if about_row(ui, item, lang) {
                            let _ = open_path_external(std::path::Path::new(item.url));
                        }
                    }
                });
            ui.add_space(10.0);
            ui.label(
                RichText::new(t.license_note)
                    .size(12.0)
                    .color(TEXT_HINT),
            );
        });
}

/// 关于页的一行开源组件：整行可点击，悬停高亮。
fn about_row(ui: &mut egui::Ui, item: &Component, lang: Lang) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
    let response = response.on_hover_text(item.url);
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 6.0, CARD_HOVER);
    }
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        item.name,
        egui::FontId::proportional(14.0),
        TEXT_MAIN,
    );
    painter.text(
        egui::pos2(rect.left() + 250.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        item.license,
        egui::FontId::proportional(12.0),
        TEXT_HINT,
    );
    painter.text(
        egui::pos2(rect.left() + 376.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        lang.usage(item.usage),
        egui::FontId::proportional(12.5),
        TEXT_SECOND,
    );
    // 右上向外箭头：悬停变主色，替代等宽的「打开」文字。
    let arrow_color = if response.hovered() { PRIMARY } else { TEXT_HINT };
    let stroke = egui::Stroke::new(1.4_f32, arrow_color);
    let tip = egui::pos2(rect.right() - 14.0, rect.center().y - 5.0);
    let tail = egui::pos2(rect.right() - 22.0, rect.center().y + 3.0);
    painter.line_segment([tail, tip], stroke);
    painter.line_segment([tip, egui::pos2(tip.x - 6.0, tip.y)], stroke);
    painter.line_segment([tip, egui::pos2(tip.x, tip.y + 6.0)], stroke);
    response.clicked()
}

/// 绘制品牌 Logo（侧栏 28px / 空状态 72px / 关于页 56px）。
fn draw_logo(ui: &mut egui::Ui, logo: &egui::TextureHandle, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().image(
        logo.id(),
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

fn log_color(level: LogLevel) -> Color32 {
    match level {
        LogLevel::Info => PRIMARY,
        LogLevel::Ok => SUCCESS,
        LogLevel::Warn => WARN,
        LogLevel::Error => DANGER,
    }
}

/// 从 JSON 值里取字符串字段。
fn text_of(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string()
}

/// 从 `write` / `verify` 报告里取失败原因。
fn report_reason(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    let inner = value
        .get("Failed")
        .or_else(|| value.get("Partial"))?;
    inner
        .get("reason")
        .and_then(|item| item.as_str())
        .map(str::to_string)
        .filter(|text| !text.trim().is_empty())
}

fn local_time(raw: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|time| time.with_timezone(&chrono::Local))
}

/// 完整时间戳，用于悬停提示。
fn full_time(raw: &str) -> String {
    match local_time(raw) {
        Some(time) => time.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => raw.to_owned(),
    }
}

/// 今天 / 昨天 / 具体日期，用于列表分组与行内相对时间。
fn day_label(raw: &str, lang: Lang) -> String {
    let Some(time) = local_time(raw) else {
        return raw.to_owned();
    };
    let today = chrono::Local::now().date_naive();
    let date = time.date_naive();
    if date == today {
        lang.texts().group_today.to_string()
    } else if Some(date) == today.pred_opt() {
        lang.texts().group_yesterday.to_string()
    } else {
        time.format("%Y-%m-%d").to_string()
    }
}

fn relative_time(raw: &str, lang: Lang) -> String {
    let Some(time) = local_time(raw) else {
        return raw.to_owned();
    };
    let today = chrono::Local::now().date_naive();
    let date = time.date_naive();
    let clock = time.format("%H:%M").to_string();
    if date == today {
        lang.today_at(&clock)
    } else if Some(date) == today.pred_opt() {
        lang.yesterday_at(&clock)
    } else {
        time.format("%m-%d %H:%M").to_string()
    }
}

/// 按字符数截断，避免长文件名挤压行内其它指标。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(max.saturating_sub(1)).collect();
    short.push('…');
    short
}

/// 设备 ID 取前 8 位显示，完整哈希放进悬停提示。
fn short_device(id: &str) -> String {
    let mut short: String = id.chars().take(8).collect();
    if id.chars().count() > 8 {
        short.push('…');
    }
    short
}

/// 批次涉及的盘符（去重，最多显示 4 个）。
fn batch_letters(record: &LedgerRecord) -> String {
    let mut letters: Vec<&str> = Vec::new();
    for disk in &record.disks {
        for letter in disk.letters.split_whitespace() {
            if !letters.contains(&letter) {
                letters.push(letter);
            }
        }
    }
    if letters.len() > 4 {
        format!("{} …", letters[..4].join(" "))
    } else {
        letters.join(" ")
    }
}

/// 批次里第一个可用的设备型号。
fn batch_model(record: &LedgerRecord) -> &str {
    record
        .disks
        .iter()
        .map(|disk| disk.model.trim())
        .find(|model| !model.is_empty())
        .unwrap_or("")
}

/// 批次里第一个可用的容量。
fn batch_capacity(record: &LedgerRecord) -> u64 {
    record
        .disks
        .iter()
        .map(|disk| disk.capacity)
        .find(|capacity| *capacity > 0)
        .unwrap_or(0)
}

/// 批次均速 = 镜像大小 × 成功盘数 / 用时。
fn batch_speed(record: &LedgerRecord) -> Option<String> {
    let seconds = elapsed_seconds(record.started, record.ended)?;
    let bytes = record.image_size.checked_mul(record.succeeded as u64)?;
    if bytes == 0 {
        return None;
    }
    Some(format!("{}/s", format_bytes((bytes as f64 / seconds) as u64)))
}

/// 历史页搜索：镜像名 / 路径 / 盘符 / 型号 / 设备 ID。
fn record_matches(record: &LedgerRecord, query: &str) -> bool {
    record.image_name.to_lowercase().contains(query)
        || record.image_path.to_lowercase().contains(query)
        || record.disks.iter().any(|disk| {
            disk.letters.to_lowercase().contains(query)
                || disk.model.to_lowercase().contains(query)
                || disk.device.to_lowercase().contains(query)
        })
}

/// 设置页分组标题。
fn group_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).strong().color(TEXT_HINT));
    ui.add_space(6.0);
}

/// 设置项：左侧标题 + 说明，右侧控件。
fn setting_row(
    ui: &mut egui::Ui,
    title: &str,
    desc: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::none()
        .fill(CARD)
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(16.0, 12.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).size(15.0).strong());
                    ui.label(RichText::new(desc).size(12.5).color(TEXT_HINT));
                });
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| control(ui),
                );
            });
        });
}

/// 开关：开启 = 主色底 + 白点靠右；关闭 = 深灰底 + 灰点靠左。
fn toggle_switch(ui: &mut egui::Ui, on: bool) -> bool {
    let size = egui::vec2(40.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let t = ui.ctx().animate_bool(response.id, on);
    let track = if on { PRIMARY } else { Color32::from_rgb(0x3A, 0x41, 0x50) };
    ui.painter().rect_filled(rect, 11.0, track);
    let knob_x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    ui.painter()
        .circle_filled(egui::pos2(knob_x, rect.center().y), 8.0, Color32::WHITE);
    response.clicked()
}

fn settings_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("settings.json"))
}

fn load_settings() -> Settings {
    settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 启动时按保留天数清理日志文件（0 = 永久保留）。
fn prune_logs(keep_days: u32) {
    if keep_days == 0 {
        return;
    }
    let Some(path) = log_file_path() else {
        return;
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    let cutoff = chrono::Local::now().date_naive()
        - chrono::Duration::days(i64::from(keep_days));
    let mut kept = String::new();
    let mut dropped = 0usize;
    for line in content.lines() {
        let date = line.get(..10).and_then(|day| {
            chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
        });
        match date {
            Some(day) if day < cutoff => dropped += 1,
            _ => {
                kept.push_str(line);
                kept.push('\n');
            }
        }
    }
    if dropped > 0 {
        let _ = std::fs::write(path, kept);
    }
}

// ---------------------------------------------------------------- 视图

impl eframe::App for BurnApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let enter = ctx.input(|input| input.key_pressed(egui::Key::Enter));
        // 快捷键：Ctrl+O 选镜像、F5 刷设备（仅在主视图生效）。
        let ctrl_o = ctx.input(|input| {
            input.key_pressed(egui::Key::O) && input.modifiers.ctrl && !input.modifiers.shift
        });
        let f5 = ctx.input(|input| input.key_pressed(egui::Key::F5));
        if self.stage == Stage::Main && !self.confirm_open {
            if ctrl_o {
                if let Some(path) = pick_image_file() {
                    self.image_path = path;
                    self.inspect_image();
                }
            }
            if f5 {
                self.rescan();
            }
        }

        // 拖拽镜像文件到窗口即可加载（与常见烧录工具一致）。
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .next()
        });
        if let Some(path) = dropped {
            if self.stage == Stage::Main {
                self.page = Page::Home;
                self.image_path = path.display().to_string();
                self.inspect_image();
            }
        }

        // G3：只有 PIN 输入期间禁粘贴。
        if self.confirm_open {
            ctx.input_mut(|input| {
                input
                    .events
                    .retain(|event| !matches!(event, egui::Event::Paste(_)));
            });
        }

        if self.confirm_open {
            if escape {
                self.cancel_confirm(self.lang().cancel_reason_esc());
            } else if enter && !self.pin_expected.is_empty() && self.pin_input == self.pin_expected
            {
                self.begin_burn();
            }
        } else if escape && self.stage == Stage::Progress {
            // G4：Esc = 取消。
            self.cancel_burn();
        }

        // 写入进行中停留在历史页时自动刷新，无需手动点「刷新」。
        if self.page == Page::History
            && self.stage == Stage::Progress
            && self.ledger_refresh_at.elapsed() >= Duration::from_secs(2)
        {
            self.load_ledger();
            self.ledger_refresh_at = Instant::now();
        }

        self.sidebar(ctx);

        // 页面切换淡入：180ms 内从 0.45 过渡到 1.0。
        if self.page != self.last_page {
            self.last_page = self.page;
            self.page_shown_at = Instant::now();
        }
        let fade_raw = (self.page_shown_at.elapsed().as_secs_f32() / 0.18).clamp(0.0, 1.0);
        let fade = fade_raw * fade_raw * (3.0 - 2.0 * fade_raw);
        if fade_raw < 1.0 {
            ctx.request_repaint();
        }

        egui::CentralPanel::default()
            .frame(page_frame())
            .show(ctx, |ui| {
                ui.set_opacity(0.45 + 0.55 * fade);
                match self.page {
                    Page::Home => match self.stage {
                        Stage::Main => self.main_view(ui),
                        Stage::Progress => self.progress_view(ui),
                        Stage::Done => self.done_view(ui),
                    },
                    Page::Logs => self.logs_page(ui),
                    Page::History => self.history_page(ui),
                    Page::Settings => self.settings_page(ui),
                    Page::About => about_page(ui, &self.logo, self.lang()),
                }
            });

        if self.confirm_open {
            self.confirm_window(ctx);
        }

        if self.stage == Stage::Progress {
            self.sample_progress();
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }
}

impl BurnApp {
    fn main_view(&mut self, root: &mut egui::Ui) {
        let lang = self.lang();
        let t = lang.texts();
        // bottom_up：动作条贴底先画，剩余空间给内容，避免内容压在动作条后面。
        root.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        // bottom_up 会在动作条下多留一段 item_spacing，压成 0 才能和内容区贴齐。
        let saved_spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing.y = 0.0;
        bar_frame().show(ui, |ui| {
            ui.horizontal(|ui| {
                let participating = self.participating_ids().len();
                let can_start = !self.scanning
                    && self.image_info.is_some()
                    && participating > 0
                    && !self.loading_image;
                let (fill, text_color) = if can_start {
                    (PRIMARY, Color32::WHITE)
                } else {
                    (PRIMARY_DISABLED, PRIMARY_DISABLED_TEXT)
                };
                let button = egui::Button::new(
                    RichText::new(t.start_write)
                        .size(18.0)
                        .strong()
                        .color(text_color),
                )
                .fill(fill)
                .min_size(egui::vec2(124.0, 36.0));
                if ui.add_enabled(can_start, button).clicked() {
                    self.request_start();
                }

                // 参与徽章：统计信息收成一个小胶囊。
                egui::Frame::none()
                    .fill(CARD_HOVER)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(lang.of_badge(participating, self.devices.len()))
                            .small());
                    });

                // 禁用原因常驻，避免“为什么点不了”。
                let reason = if !can_start {
                    Some(if self.loading_image {
                        t.reason_loading
                    } else if self.image_info.is_none() {
                        t.reason_no_image
                    } else if self.scanning {
                        t.reason_scanning
                    } else if self.devices.is_empty() {
                        t.reason_no_disk
                    } else {
                        t.reason_no_target
                    })
                } else {
                    None
                };
                if let Some(reason) = reason {
                    ui.label(RichText::new(reason).small().color(TEXT_SECOND));
                }

                // 右侧只放真正的错误，避免与左侧禁用原因重复。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(notice) = &self.notice {
                        if Some(notice.as_str()) != reason {
                            ui.colored_label(DANGER, notice);
                        }
                    } else if can_start {
                        ui.label(RichText::new(t.ready).small().color(SUCCESS));
                    }
                });
            });
            });

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = saved_spacing;
            // 与日志/历史/设置页的标题留白对齐，避免镜像卡贴着标题栏。
            ui.add_space(12.0);
            // ① 镜像卡：图标 + 路径 + 浏览/加载 收进一张卡；拖入文件时边框变主色。
            let drag_hover = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
            egui::Frame::none()
                .fill(CARD)
                .rounding(8.0)
                .inner_margin(egui::Margin::same(12.0))
                .stroke(if drag_hover {
                    egui::Stroke::new(1.5_f32, PRIMARY)
                } else {
                    egui::Stroke::NONE
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        draw_disc_icon(ui, drag_hover);
                        let gap = ui.spacing().item_spacing.x;
                        let trailing = 68.0 + 56.0 + gap * 2.0;
                        let width = (ui.available_width() - trailing).max(160.0);
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.image_path)
                                .hint_text(t.hint_image_path)
                                .desired_width(width),
                        );
                        let entered = response.has_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if entered {
                            self.inspect_image();
                        }
                        if ui
                            .add(secondary_button(t.browse, egui::vec2(68.0, CONTROL_H)))
                            .on_hover_text(t.hint_image_ctrl)
                            .clicked()
                        {
                            if let Some(path) = pick_image_file() {
                                self.image_path = path;
                                self.inspect_image();
                            }
                        }
                        let can_load = !self.image_path.trim().is_empty() && !self.loading_image;
                        // 禁用时手动压暗（显式 fill 不会被 add_enabled 自动调暗）。
                        let (load_fill, load_text) = if can_load {
                            (PRIMARY, Color32::WHITE)
                        } else {
                            (PRIMARY_DISABLED, PRIMARY_DISABLED_TEXT)
                        };
                        let load_button = egui::Button::new(
                            RichText::new(t.load).strong().color(load_text),
                        )
                        .fill(load_fill)
                        .min_size(egui::vec2(56.0, CONTROL_H));
                        if ui.add_enabled(can_load, load_button).clicked() {
                            self.inspect_image();
                        }
                    });

                    // 提示 / 摘要 / 错误统一缩进到输入框左边缘（图标 30 + 间距 8）。
                    const ROW_INDENT: f32 = 38.0;
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(ROW_INDENT);
                        if drag_hover {
                            ui.label(RichText::new(t.drop_to_load).small().color(PRIMARY));
                        } else {
                            ui.label(RichText::new(t.drag_hint).small().color(TEXT_HINT));
                        }
                    });

                    if self.loading_image {
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.add_space(ROW_INDENT);
                            ui.label(RichText::new(t.reading_sha).weak());
                        });
                    }
                    if let Some(info) = &self.image_info {
                        let hash = info.sha256.get(..16).unwrap_or(info.sha256.as_str());
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.add_space(ROW_INDENT);
                            ui.label(RichText::new(&info.name).strong().size(15.0));
                            ui.label(RichText::new(format_bytes(info.size)).size(15.0));
                            ui.label(
                                RichText::new(format!("SHA-256 {hash}…"))
                                    .small()
                                    .color(TEXT_HINT),
                            );
                        });
                    }
                    if let Some(error) = &self.image_error {
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.add_space(ROW_INDENT);
                            ui.colored_label(DANGER, error);
                        });
                    }
                });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            if self.scanning {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t.enumerating).weak());
                });
            } else if self.devices.is_empty() {
                self.empty_state(ui);
            }

            let mut action = None;
            let lang = self.lang();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for device in &self.devices {
                        let excluded = self.excluded.contains(device.id.as_str());
                        if let Some(next) =
                            device_card(ui, device, excluded, None, 0, None, lang)
                        {
                            action = Some(next);
                        }
                        ui.add_space(8.0);
                    }
                });

            match action {
                Some(RowAction::Open(letter)) => {
                    self.log(self.lang().open_explorer_log(&letter));
                    open_in_explorer(&letter);
                }
                Some(RowAction::Toggle(id)) => self.toggle_excluded(&id),
                None => {}
            }
        });
        });
    }

    /// 空状态：三步引导 + 实心「扫描设备」。
    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let mut rescan = false;
        let t = self.lang().texts();
        // 已移除中部 Logo：按剩余内容高度重新垂直居中，避免上方留白偏大。
        const CONTENT_H: f32 = 210.0;
        let top = ((ui.available_height() - CONTENT_H) * 0.5).max(24.0);
        // ② 三步流程引导：选镜像 → 插 U 盘 → 开始写入，一眼看清动线。
        let steps = [
            (t.step1, t.step1_sub),
            (t.step2, t.step2_sub),
            (t.step3, t.step3_sub),
        ];
        ui.vertical_centered(|ui| {
            ui.add_space(top);
            ui.label(RichText::new(t.no_devices).size(19.0).strong());
            ui.add_space(24.0);
            // 固定卡宽 + 手动算左内边距，让三步引导整体在内容区水平居中。
            // 卡片实际占位 = 内容宽 + 左右内边距（16×2）。
            const CARD_W: f32 = 170.0;
            const CARD_PAD: f32 = 16.0;
            const ARROW_W: f32 = 14.0;
            let gap = ui.spacing().item_spacing.x;
            let card_total = CARD_W + CARD_PAD * 2.0;
            let total = card_total * 3.0 + 2.0 * (ARROW_W + gap * 2.0);
            let pad = ((ui.available_width() - total) / 2.0).max(0.0);
            ui.horizontal(|ui| {
                ui.add_space(pad);
                for (index, (title, sub)) in steps.iter().enumerate() {
                    egui::Frame::none()
                        .fill(CARD)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(CARD_PAD, 10.0))
                        .show(ui, |ui| {
                            ui.set_width(CARD_W);
                            // 固定卡宽 + 交叉轴居中，避免被内容撑满整行。
                            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                                let (badge, _) = ui
                                    .allocate_exact_size(
                                        egui::vec2(28.0, 28.0),
                                        egui::Sense::hover(),
                                    );
                                let painter = ui.painter();
                                painter.circle_filled(badge.center(), 14.0, PRIMARY);
                                painter.text(
                                    badge.center(),
                                    egui::Align2::CENTER_CENTER,
                                    &(index + 1).to_string(),
                                    egui::FontId::proportional(15.0),
                                    Color32::WHITE,
                                );
                                ui.add_space(4.0);
                                ui.label(RichText::new(*title).strong().size(15.0));
                                ui.label(RichText::new(*sub).small().color(TEXT_HINT));
                            });
                        });
                    if index + 1 < steps.len() {
                        // 步骤之间的箭头，固定宽度保证三张卡等距。
                        let (arrow_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ARROW_W, 20.0),
                            egui::Sense::hover(),
                        );
                        icons::paint(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                arrow_rect.center(),
                                egui::vec2(ARROW_W, ARROW_W),
                            ),
                            Icon::ArrowRight,
                            TEXT_SECOND,
                        );
                    }
                }
            });
            ui.add_space(24.0);
            if icon_button(ui, Icon::Search, t.scan_devices, BtnKind::Primary, 120.0)
                .on_hover_text(t.scan_devices_tip)
                .clicked()
            {
                rescan = true;
            }
        });
        if rescan {
            self.rescan();
        }
    }

    fn progress_view(&mut self, root: &mut egui::Ui) {
        let lang = self.lang();
        let t = lang.texts();
        let snapshot = self.control.as_ref().map(|control| control.snapshot());
        let per_disk = self
            .control
            .as_ref()
            .map(|control| control.disk_progress())
            .unwrap_or_default();
        let state = snapshot.map(|value| value.state).unwrap_or(BurnState::Preparing);
        let done = per_disk
            .iter()
            .filter(|disk| disk.phase == DiskPhase::Done)
            .count();
        let failed = per_disk
            .iter()
            .filter(|disk| disk.phase == DiskPhase::Failed)
            .count();
        let total = per_disk.len();

        egui::TopBottomPanel::bottom("progress_actions")
            .frame(bar_frame())
            .show_inside(root, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(t.cancel_batch)
                                .size(18.0)
                                .color(Color32::WHITE),
                        )
                        .fill(DANGER)
                        .min_size(egui::vec2(110.0, 36.0)),
                    )
                    .on_hover_text(t.cancel_batch_tip)
                    .clicked()
                {
                    self.cancel_burn();
                }
                ui.label(
                    RichText::new(lang.state_line(lang.burn_state(state), done, total))
                        .strong(),
                );
                if failed > 0 {
                    ui.colored_label(DANGER, lang.failed_n(failed));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(t.esc_abort_hint).small().color(TEXT_HINT));
                });
            });
        });

        root.vertical(|ui| {
            if let Some(snapshot) = snapshot {
                draw_progress(ui, snapshot, self.image_info.as_ref().map(|info| info.size), lang);
            } else {
                ui.label(RichText::new(t.preparing).weak());
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            // C8：写入阶段不提供排除，只能中止。
            let image_size = self.image_info.as_ref().map(|info| info.size).unwrap_or(0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for device in self.participating_ids() {
                        let progress = per_disk
                            .iter()
                            .find(|item| item.device_id == device.id.as_str());
                        let speed = self.speeds.get(device.id.as_str()).copied();
                        device_card(ui, device, false, progress, image_size, speed, lang);
                        ui.add_space(8.0);
                    }
                });
        });
    }

    fn done_view(&mut self, root: &mut egui::Ui) {
        let lang = self.lang();
        let t = lang.texts();
        egui::TopBottomPanel::bottom("done_actions")
            .frame(bar_frame())
            .show_inside(root, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add(primary_button(t.back_rescan, egui::vec2(150.0, 36.0)))
                    .clicked()
                {
                    self.back_to_main();
                }
                if ui.add(ghost_button(t.view_ledger, TEXT_SECOND)).clicked() {
                    self.load_ledger();
                    self.page = Page::History;
                }
            });
        });

        root.vertical(|ui| {
            let Some(outcome) = &self.outcome else {
                ui.label(t.no_result);
                return;
            };

            let (title, color) = match outcome.state {
                BurnState::Completed => (t.all_done, Color32::from_rgb(60, 175, 100)),
                BurnState::Aborted => (t.aborted, Color32::from_rgb(225, 85, 85)),
                BurnState::Idle
                | BurnState::Preparing
                | BurnState::Writing
                | BurnState::Verifying => (lang.burn_state(outcome.state), Color32::GRAY),
            };
            ui.add_space(4.0);
            ui.heading(RichText::new(title).color(color));
            if let Some(error) = &outcome.batch_error {
                ui.colored_label(log_color(LogLevel::Error), error);
            }
            ui.add_space(16.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for disk in &outcome.disks {
                        let fill = if disk.error.is_some() {
                            Color32::from_rgb(44, 30, 30)
                        } else {
                            Color32::from_gray(34)
                        };
                        egui::Frame::none()
                            .fill(fill)
                            .rounding(egui::Rounding::same(6.0))
                            .inner_margin(egui::Margin::same(8.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(disk.device.to_string())
                                            .monospace()
                                            .size(14.0),
                                    );
                                    ui.label(
                                        RichText::new(lang.write_report(&disk.write)).size(15.0),
                                    );
                                    ui.label(
                                        RichText::new(lang.verify_report(&disk.verify))
                                            .size(15.0)
                                            .weak(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let ok = matches!(
                                                disk.verify,
                                                VerifyReport::Passed
                                            );
                                            let (text, color) = if ok {
                                                (t.passed, Color32::from_rgb(60, 175, 100))
                                            } else {
                                                (t.not_passed, Color32::from_rgb(225, 85, 85))
                                            };
                                            ui.colored_label(color, RichText::new(text).strong());
                                        },
                                    );
                                });
                                // G4：失败原因常驻。
                                if let Some(error) = &disk.error {
                                    ui.horizontal(|ui| {
                                        ui.add_space(12.0);
                                        ui.colored_label(log_color(LogLevel::Error), error);
                                    });
                                }
                            });
                        ui.add_space(8.0);
                    }
                });
        });
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let lang = self.lang();
        let t = lang.texts();
        let mut close = false;
        let mut start = false;

        egui::Window::new(t.confirm_title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            // 嵌套圆角：控件 6 + 弹窗内边距 4 = 弹窗 10。
            .frame(
                egui::Frame::popup(&ctx.style())
                    .rounding(egui::Rounding::same(10.0))
                    .inner_margin(4.0),
            )
            .show(ctx, |ui| {
                ui.set_min_width(560.0);
                ui.label(t.confirm_warning);
                ui.add_space(4.0);

                let mut toggled = None;
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for device in &self.devices {
                        let excluded = self.excluded.contains(device.id.as_str());
                        if let Some(RowAction::Toggle(id)) =
                            device_card(ui, device, excluded, None, 0, None, lang)
                        {
                            toggled = Some(id);
                        }
                        ui.add_space(8.0);
                    }
                });
                if let Some(id) = toggled {
                    self.toggle_excluded(&id);
                }

                ui.add_space(16.0);
                ui.separator();

                let pin_ok =
                    !self.pin_expected.is_empty() && self.pin_input == self.pin_expected;
                if !self.pin_expected.is_empty() {
                    ui.label(t.pin_prompt);
                    ui.label(
                        RichText::new(&self.pin_expected)
                            .size(34.0)
                            .monospace()
                            .strong()
                            .color(Color32::from_rgb(210, 60, 60)),
                    );
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.pin_input)
                            .char_limit(6)
                            .desired_width(160.0),
                    );
                    if self.pin_focus {
                        response.request_focus();
                        self.pin_focus = false;
                    }
                    if response.changed() {
                        // G3：只保留数字字符。
                        self.pin_input.retain(|character| character.is_ascii_digit());
                    }
                    if pin_ok {
                        ui.colored_label(Color32::from_rgb(40, 150, 70), t.pin_correct);
                    }
                }

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.button(t.cancel).clicked() {
                        close = true;
                    }
                    // C3：输对 PIN 后「确认」才可用。
                    if ui
                        .add_enabled(pin_ok, egui::Button::new(t.confirm_write))
                        .clicked()
                    {
                        start = true;
                    }
                });
            });

        if close {
            self.cancel_confirm(lang.cancel_reason());
        }
        if start {
            self.begin_burn();
        }
    }
}

// ---------------------------------------------------------------- 组件

enum RowAction {
    Open(String),
    Toggle(String),
}

/// 卡片式盘行：勾选框（参与）+ 盘符 + 卷标 + 文件系统 + 容量 + 三态，写入时附逐盘进度。
#[allow(clippy::too_many_arguments)]
fn device_card(
    ui: &mut egui::Ui,
    device: &DeviceCandidate,
    excluded: bool,
    progress: Option<&DiskProgress>,
    image_size: u64,
    speed: Option<f64>,
    lang: Lang,
) -> Option<RowAction> {
    let t = lang.texts();
    let mut action = None;
    let fill = if excluded {
        Color32::from_gray(26)
    } else {
        Color32::from_gray(34)
    };

    egui::Frame::none()
        .fill(fill)
        .rounding(egui::Rounding::same(6.0))
        .inner_margin(egui::Margin::same(8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(CONTROL_H);

                // C7：默认全部参与，取消勾选即排除。
                let mut checked = !excluded;
                if ui
                    .checkbox(&mut checked, "")
                    .on_hover_text(t.card_participate_tip)
                    .changed()
                {
                    action = Some(RowAction::Toggle(device.id.as_str().to_owned()));
                }

                let letters = device.letters_display();
                if letters.is_empty() {
                    ui.label(RichText::new(t.no_letter).weak().size(14.0));
                } else {
                    for letter in letters.split_whitespace() {
                        if ui
                            .add(
                                egui::Button::new(RichText::new(letter).strong().size(16.0))
                                    .min_size(egui::vec2(44.0, 26.0)),
                            )
                            .on_hover_text(t.card_open_tip)
                            .clicked()
                        {
                            action = Some(RowAction::Open(letter.to_owned()));
                        }
                    }
                }

                let label = device.primary_label();
                ui.label(
                    RichText::new(if label.is_empty() { t.no_label } else { label })
                        .size(17.0)
                        .strong(),
                );

                let file_system = device.primary_file_system();
                if !file_system.is_empty() {
                    ui.label(RichText::new(file_system).size(15.0));
                }

                ui.label(
                    RichText::new(lang.used_of(
                        &format_bytes(device.used_bytes()),
                        &format_bytes(device.capacity_bytes),
                    ))
                    .size(15.0),
                );

                ui.label(
                    RichText::new(lang.disk_state(device.state))
                        .color(state_color(device.state))
                        .strong()
                        .size(15.0),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if excluded {
                        ui.label(RichText::new(t.excluded).weak());
                    }
                    ui.label(RichText::new(&device.bus).small().weak());
                });
            });

            // B6：无法判断时展示分区表、文件系统签名与熵值。
            if device.state == DiskState::Unknown {
                let signature = device
                    .partitions
                    .iter()
                    .filter_map(|partition| partition.signature.as_deref())
                    .find(|value| !value.is_empty())
                    .unwrap_or(t.no_signature);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(lang.detail_line(
                            lang.table_kind(device.table),
                            device.partitions.len(),
                            signature,
                            device.entropy.unwrap_or(0.0) as f64,
                        ))
                        .small()
                        .weak(),
                    );
                });
            }

            // 逐盘进度：进度条 + 百分比 + 速度 + 阶段。
            if let Some(progress) = progress {
                let total = if image_size > 0 {
                    image_size
                } else {
                    device.capacity_bytes
                };
                let fraction = if total == 0 {
                    0.0
                } else {
                    (progress.written_bytes as f64 / total as f64).clamp(0.0, 1.0) as f32
                };
                ui.add_space(4.0);
                // 进度条平滑推进，避免整段跳变。
                let shown = ui.ctx().animate_value_with_time(
                    egui::Id::new(("disk_progress", &device.id)),
                    fraction,
                    0.3,
                );
                ui.add(
                    egui::ProgressBar::new(shown)
                        .show_percentage()
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            format_bytes(progress.written_bytes),
                            format_bytes(total)
                        ))
                        .small()
                        .weak(),
                    );
                    if let Some(value) = speed {
                        if value > 0.0 {
                            ui.label(
                                RichText::new(format!("{:.1} MB/s", value / 1_048_576.0))
                                    .small()
                                    .weak(),
                            );
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(lang.disk_phase(progress.phase))
                                .small()
                                .color(phase_color(progress.phase)),
                        );
                    });
                });
            }
        });

    action
}

fn phase_color(phase: DiskPhase) -> Color32 {
    match phase {
        DiskPhase::Waiting => Color32::from_gray(150),
        DiskPhase::Writing => Color32::from_rgb(90, 150, 230),
        DiskPhase::Verifying => Color32::from_rgb(200, 160, 60),
        DiskPhase::Done => Color32::from_rgb(60, 175, 100),
        DiskPhase::Failed => Color32::from_rgb(225, 85, 85),
    }
}

/// 镜像卡左侧的圆盘图标（拖拽悬停时变主色）。
fn draw_disc_icon(ui: &mut egui::Ui, accent: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
    let color = if accent { PRIMARY } else { Color32::from_gray(120) };
    icons::paint(ui.painter(), rect, Icon::Cd, color);
}

/// 「打开日志」：在资源管理器中打开安装目录下的 logs 文件夹。
fn open_logs_dir() -> bool {
    let Some(dir) = logs_dir() else {
        return false;
    };
    let _ = std::fs::create_dir_all(&dir);
    open_path_external(&dir)
}

fn draw_progress(
    ui: &mut egui::Ui,
    snapshot: BurnSnapshot,
    image_size: Option<u64>,
    lang: Lang,
) {
    let total = if snapshot.total_bytes > 0 {
        snapshot.total_bytes
    } else {
        image_size.unwrap_or(0)
    };
    let fraction = if total == 0 {
        0.0
    } else {
        (snapshot.written_bytes as f64 / total as f64).clamp(0.0, 1.0) as f32
    };
    // 总进度条同样做平滑推进。
    let shown = ui
        .ctx()
        .animate_value_with_time(ui.id().with("batch_progress"), fraction, 0.3);
    ui.add(
        egui::ProgressBar::new(shown)
            .show_percentage()
            .desired_width(f32::INFINITY),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(lang.written_of(
            &format_bytes(snapshot.written_bytes),
            &format_bytes(total),
        ))
        .small()
        .weak(),
    );
}

fn state_color(state: DiskState) -> Color32 {
    match state {
        DiskState::HasContent => Color32::from_rgb(200, 120, 40),
        DiskState::Empty => Color32::from_rgb(40, 150, 70),
        DiskState::Unknown => Color32::from_rgb(190, 60, 60),
    }
}

fn open_in_explorer(letter: &str) {
    if !letter.is_empty() {
        open_path_external(std::path::Path::new(&format!("{letter}\\")));
    }
}

/// 字节数的可读文本（i18n 文案方法也会调用）。
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// C2：每次确认弹窗重新生成 6 位 PIN。
fn generate_pin() -> String {
    use sha2::{Digest, Sha256};

    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(elapsed.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update((&elapsed as *const u128 as usize).to_le_bytes());
    let digest = hasher.finalize();
    let value = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) % 1_000_000;
    format!("{value:06}")
}
