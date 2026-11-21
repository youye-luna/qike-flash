//! IconPark 图标（ByteDance IconPark，`@icon-park/svg` 1.4.2，Apache-2.0）。
//!
//! 只内联本程序用到的图标路径，保持 48×48 视口、4px 线条的原始设计，
//! 由 egui painter 以折线方式描边绘制，不引入 SVG/字体依赖。

use eframe::egui::{self, Color32, Pos2, Shape, Stroke};

/// 用到的图标。
#[derive(Debug, Clone, Copy)]
pub enum Icon {
    Home,
    Log,
    History,
    Setting,
    Info,
    FolderOpen,
    Copy,
    Delete,
    Refresh,
    Download,
    Search,
    ArrowRight,
    Cd,
    /// 展开指示（收起态）。
    ChevronRight,
    /// 展开指示（展开态）。
    ChevronDown,
    /// 成功勾选。
    Check,
    /// 失败叉号。
    Close,
}

/// 图标数据结构：路径 + 实心/空心矩形 + 圆。
struct Def {
    paths: &'static [&'static str],
    /// (x, y, w, h, 是否填充)
    rects: &'static [(f32, f32, f32, f32, bool)],
    /// (cx, cy, r, 是否填充)
    circles: &'static [(f32, f32, f32, bool)],
}

const EMPTY_RECTS: &[(f32, f32, f32, f32, bool)] = &[];
const EMPTY_CIRCLES: &[(f32, f32, f32, bool)] = &[];

fn def(icon: Icon) -> Def {
    match icon {
        Icon::Home => Def {
            paths: &[
                "M9 18V42H39V18L24 6L9 18Z",
                "M19 29V42H29V29H19Z",
                "M9 42H39",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Log => Def {
            paths: &[
                "M35 10V4H8C7.44772 4 7 4.44772 7 5V38H13",
                "M21 22H33",
                "M21 30H33",
            ],
            rects: &[(13.0, 10.0, 28.0, 34.0, false)],
            circles: EMPTY_CIRCLES,
        },
        Icon::History => Def {
            paths: &[
                "M5.81836 6.72729V14H13.0911",
                "M4 24C4 35.0457 12.9543 44 24 44V44C35.0457 44 44 35.0457 44 24C44 12.9543 35.0457 4 24 4C16.598 4 10.1351 8.02111 6.67677 13.9981",
                "M24.005 12L24.0038 24.0088L32.4832 32.4882",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Setting => Def {
            paths: &[
                "M36.686 15.171C37.9364 16.9643 38.8163 19.0352 39.2147 21.2727H44V26.7273H39.2147C38.8163 28.9648 37.9364 31.0357 36.686 32.829L40.0706 36.2137L36.2137 40.0706L32.829 36.686C31.0357 37.9364 28.9648 38.8163 26.7273 39.2147V44H21.2727V39.2147C19.0352 38.8163 16.9643 37.9364 15.171 36.686L11.7863 40.0706L7.92939 36.2137L11.314 32.829C10.0636 31.0357 9.18372 28.9648 7.92939 26.7273H4V21.2727H8.78533C9.18372 19.0352 10.0636 16.9643 11.314 15.171L7.92939 11.7863L11.7863 7.92939L15.171 11.314C16.9643 10.0636 19.0352 9.18372 21.2727 8.78533V4H26.7273V8.78533C28.9648 9.18372 31.0357 10.0636 32.829 11.314L36.2137 7.92939L40.0706 11.7863L36.686 15.171Z",
                "M24 29C26.7614 29 29 26.7614 29 24C29 21.2386 26.7614 19 24 19C21.2386 19 19 21.2386 19 24C19 26.7614 21.2386 29 24 29Z",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Info => Def {
            paths: &[
                "M24 44C29.5228 44 34.5228 41.7614 38.1421 38.1421C41.7614 34.5228 44 29.5228 44 24C44 18.4772 41.7614 13.4772 38.1421 9.85786C34.5228 6.23858 29.5228 4 24 4C18.4772 4 13.4772 6.23858 9.85786 9.85786C6.23858 13.4772 4 18.4772 4 24C4 29.5228 6.23858 34.5228 9.85786 38.1421C13.4772 41.7614 18.4772 44 24 44Z",
                "M24.5 34V20H23.5H22.5",
                "M21 34H28",
            ],
            rects: EMPTY_RECTS,
            circles: &[(24.0, 13.5, 2.5, true)],
        },
        Icon::FolderOpen => Def {
            paths: &[
                "M4 9V41L9 21H39.5V15C39.5 13.8954 38.6046 13 37.5 13H24L19 7H6C4.89543 7 4 7.89543 4 9Z",
                "M40 41L44 21H8.8125L4 41H40Z",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Copy => Def {
            paths: &[
                "M13 12.4316V7.8125C13 6.2592 14.2592 5 15.8125 5H40.1875C41.7408 5 43 6.2592 43 7.8125V32.1875C43 33.7408 41.7408 35 40.1875 35H35.5163",
                "M32.1875 13H7.8125C6.2592 13 5 14.2592 5 15.8125V40.1875C5 41.7408 6.2592 43 7.8125 43H32.1875C33.7408 43 35 41.7408 35 40.1875V15.8125C35 14.2592 33.7408 13 32.1875 13Z",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Delete => Def {
            paths: &[
                "M9 10V44H39V10H9Z",
                "M20 20V33",
                "M28 20V33",
                "M4 10H44",
                "M16 10L19.289 4H28.7771L32 10H16Z",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Refresh => Def {
            paths: &[
                "M42 8V24",
                "M6 24L6 40",
                "M42 24C42 14.0589 33.9411 6 24 6C18.9145 6 14.3216 8.10896 11.0481 11.5M6 24C6 33.9411 14.0589 42 24 42C28.8556 42 33.2622 40.0774 36.5 36.9519",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Download => Def {
            paths: &[
                "M6 24.0083V42H42V24",
                "M33 23L24 32L15 23",
                "M23.9917 6V32",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Search => Def {
            paths: &[
                "M21 38C30.3888 38 38 30.3888 38 21C38 11.6112 30.3888 4 21 4C11.6112 4 4 11.6112 4 21C4 30.3888 11.6112 38 21 38Z",
                "M26.657 14.3431C25.2093 12.8954 23.2093 12 21.0001 12C18.791 12 16.791 12.8954 15.3433 14.3431",
                "M33.2216 33.2217L41.7069 41.707",
            ],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::ArrowRight => Def {
            paths: &["M41.9999 24H5.99994", "M30 12L42 24L30 36"],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Cd => Def {
            paths: &["M13 24C13 17.9249 17.9249 13 24 13"],
            rects: EMPTY_RECTS,
            circles: &[(24.0, 24.0, 18.0, false), (24.0, 24.0, 5.0, false)],
        },
        Icon::ChevronRight => Def {
            paths: &["M19 14L29 24L19 34"],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::ChevronDown => Def {
            paths: &["M14 19L24 29L34 19"],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Check => Def {
            paths: &["M9 25L19 35L39 13"],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
        Icon::Close => Def {
            paths: &["M10 10L38 38", "M38 10L10 38"],
            rects: EMPTY_RECTS,
            circles: EMPTY_CIRCLES,
        },
    }
}

/// 在 `rect` 内绘制图标（等比缩放，线条颜色为 `color`）。
pub fn paint(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    let size = rect.width().min(rect.height());
    if size <= 0.0 {
        return;
    }
    let scale = size / 48.0;
    let origin = egui::pos2(
        rect.center().x - size * 0.5,
        rect.center().y - size * 0.5,
    );
    // IconPark 原始线宽 4（48 视口），缩放到目标尺寸后不小于 1px。
    let stroke = Stroke::new((4.0 * scale).max(1.0), color);
    let def = def(icon);

    for d in def.paths {
        for points in flatten(d, origin, scale) {
            if points.len() >= 2 {
                painter.add(Shape::line(points, stroke));
            }
        }
    }
    for &(x, y, w, h, filled) in def.rects {
        let r = egui::Rect::from_min_size(
            egui::pos2(origin.x + x * scale, origin.y + y * scale),
            egui::vec2(w * scale, h * scale),
        );
        if filled {
            painter.rect_filled(r, 0.0, color);
        } else {
            painter.rect_stroke(r, 0.0, stroke);
        }
    }
    for &(cx, cy, radius, filled) in def.circles {
        let center = egui::pos2(origin.x + cx * scale, origin.y + cy * scale);
        if filled {
            painter.circle_filled(center, radius * scale, color);
        } else {
            painter.circle_stroke(center, radius * scale, stroke);
        }
    }
}

// ---------------------------------------------------------------- SVG 路径解析

enum Token {
    Command(u8),
    Number(f32),
}

fn tokenize(d: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let bytes = d.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphabetic() {
            tokens.push(Token::Command(byte));
            index += 1;
            continue;
        }
        if byte == b' ' || byte == b',' || byte == b'\n' || byte == b'\r' || byte == b'\t' {
            index += 1;
            continue;
        }
        // 数字：可选符号 + 数字 + 可选小数 + 可选指数
        let start = index;
        if byte == b'-' || byte == b'+' {
            index += 1;
        }
        while index < bytes.len()
            && (bytes[index].is_ascii_digit() || bytes[index] == b'.')
        {
            index += 1;
        }
        if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
            index += 1;
            if index < bytes.len() && (bytes[index] == b'-' || bytes[index] == b'+') {
                index += 1;
            }
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }
        if let Ok(value) = d[start..index].parse::<f32>() {
            tokens.push(Token::Number(value));
        }
    }
    tokens
}

/// 把路径展开成若干折线（已按 origin/scale 变换到屏幕坐标）。
fn flatten(d: &str, origin: Pos2, scale: f32) -> Vec<Vec<Pos2>> {
    let tokens = tokenize(d);
    let mut index = 0usize;
    let mut result: Vec<Vec<Pos2>> = Vec::new();
    let mut current: Vec<Pos2> = Vec::new();
    let to_screen = |x: f32, y: f32| Pos2::new(origin.x + x * scale, origin.y + y * scale);

    let mut cursor = (0.0f32, 0.0f32);
    let mut sub_start = (0.0f32, 0.0f32);
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut command = b'M';

    let number = |tokens: &[Token], index: &mut usize| -> Option<f32> {
        match tokens.get(*index) {
            Some(Token::Number(value)) => {
                *index += 1;
                Some(*value)
            }
            _ => None,
        }
    };

    while index < tokens.len() {
        if let Token::Command(cmd) = tokens[index] {
            command = cmd;
            index += 1;
        }
        let relative = command.is_ascii_lowercase();
        let upper = command.to_ascii_uppercase();
        match upper {
            b'Z' => {
                if !current.is_empty() {
                    current.push(to_screen(sub_start.0, sub_start.1));
                    result.push(std::mem::take(&mut current));
                }
                cursor = sub_start;
                last_ctrl = None;
            }
            b'M' | b'L' => {
                let (Some(mut x), Some(mut y)) =
                    (number(&tokens, &mut index), number(&tokens, &mut index))
                else {
                    break;
                };
                if relative {
                    x += cursor.0;
                    y += cursor.1;
                }
                if upper == b'M' {
                    if !current.is_empty() {
                        result.push(std::mem::take(&mut current));
                    }
                    sub_start = (x, y);
                    current.push(to_screen(x, y));
                } else {
                    current.push(to_screen(x, y));
                }
                cursor = (x, y);
                last_ctrl = None;
            }
            b'H' | b'V' => {
                let Some(mut value) = number(&tokens, &mut index) else {
                    break;
                };
                if upper == b'H' {
                    if relative {
                        value += cursor.0;
                    }
                    cursor.0 = value;
                } else {
                    if relative {
                        value += cursor.1;
                    }
                    cursor.1 = value;
                }
                current.push(to_screen(cursor.0, cursor.1));
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let mut nums = [0.0f32; 6];
                if upper == b'C' {
                    for slot in nums.iter_mut() {
                        match number(&tokens, &mut index) {
                            Some(value) => *slot = value,
                            None => return result,
                        }
                    }
                    let (c1x, c1y) = (nums[0], nums[1]);
                    let mut points = cubic(
                        cursor,
                        (c1x, c1y),
                        (nums[2], nums[3]),
                        (nums[4], nums[5]),
                        scale,
                        origin,
                    );
                    current.append(&mut points);
                    last_ctrl = Some((nums[2], nums[3]));
                    cursor = (nums[4], nums[5]);
                } else {
                    for slot in nums.iter_mut().take(4) {
                        match number(&tokens, &mut index) {
                            Some(value) => *slot = value,
                            None => return result,
                        }
                    }
                    let c1 = reflected(cursor, last_ctrl);
                    let mut points = cubic(
                        cursor,
                        c1,
                        (nums[0], nums[1]),
                        (nums[2], nums[3]),
                        scale,
                        origin,
                    );
                    current.append(&mut points);
                    last_ctrl = Some((nums[0], nums[1]));
                    cursor = (nums[2], nums[3]);
                }
            }
            b'Q' => {
                let mut nums = [0.0f32; 4];
                for slot in nums.iter_mut() {
                    match number(&tokens, &mut index) {
                        Some(value) => *slot = value,
                        None => return result,
                    }
                }
                let mut points =
                    quad(cursor, (nums[0], nums[1]), (nums[2], nums[3]), scale, origin);
                current.append(&mut points);
                last_ctrl = Some((nums[0], nums[1]));
                cursor = (nums[2], nums[3]);
            }
            _ => {
                // 不支持的命令（如 A/T）直接跳过其参数，避免死循环。
                while let Token::Number(_) = tokens.get(index).unwrap_or(&Token::Command(0)) {
                    index += 1;
                }
                last_ctrl = None;
            }
        }
        // 相对坐标在 M/L/H/V/C/Q 的连写里要基于更新后的 cursor；
        // 上面的实现已按绝对坐标推进，这里不再重复处理。
        if matches!(tokens.get(index), Some(Token::Command(_))) && upper == b'Z' {
            continue;
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn reflected(cursor: (f32, f32), last: Option<(f32, f32)>) -> (f32, f32) {
    match last {
        Some((x, y)) => (2.0 * cursor.0 - x, 2.0 * cursor.1 - y),
        None => cursor,
    }
}

fn cubic(
    from: (f32, f32),
    c1: (f32, f32),
    c2: (f32, f32),
    to: (f32, f32),
    scale: f32,
    origin: Pos2,
) -> Vec<Pos2> {
    const STEPS: usize = 8;
    let mut points = Vec::with_capacity(STEPS);
    for step in 1..=STEPS {
        let t = step as f32 / STEPS as f32;
        let mt = 1.0 - t;
        let x = mt * mt * mt * from.0
            + 3.0 * mt * mt * t * c1.0
            + 3.0 * mt * t * t * c2.0
            + t * t * t * to.0;
        let y = mt * mt * mt * from.1
            + 3.0 * mt * mt * t * c1.1
            + 3.0 * mt * t * t * c2.1
            + t * t * t * to.1;
        points.push(Pos2::new(origin.x + x * scale, origin.y + y * scale));
    }
    points
}

fn quad(
    from: (f32, f32),
    control: (f32, f32),
    to: (f32, f32),
    scale: f32,
    origin: Pos2,
) -> Vec<Pos2> {
    const STEPS: usize = 8;
    let mut points = Vec::with_capacity(STEPS);
    for step in 1..=STEPS {
        let t = step as f32 / STEPS as f32;
        let mt = 1.0 - t;
        let x = mt * mt * from.0 + 2.0 * mt * t * control.0 + t * t * to.0;
        let y = mt * mt * from.1 + 2.0 * mt * t * control.1 + t * t * to.1;
        points.push(Pos2::new(origin.x + x * scale, origin.y + y * scale));
    }
    points
}
