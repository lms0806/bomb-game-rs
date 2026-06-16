use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::core::{w, PCSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DeleteObject, EndPaint, FillRect, SelectObject, HBRUSH, HDC, HGDIOBJ, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExA, DefWindowProcA, DispatchMessageA, GetMessageA, LoadCursorW, PostQuitMessage,
    RegisterClassA, SetTimer, SetWindowTextW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, IDC_ARROW, MSG, SW_SHOW, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_PAINT, WM_TIMER,
    WNDCLASSA, WS_OVERLAPPEDWINDOW,
};

// 간단한 크레이지아케이드 스타일 폭탄 게임 예제
// - 방향키로 플레이어 이동
// - Space 로 폭탄 설치
// - 폭탄은 잠시 후 십자 모양으로 폭발

const CELL_SIZE: i32 = 40;
const GRID_WIDTH: i32 = 13;
const GRID_HEIGHT: i32 = 11;

const TIMER_ID: usize = 1;
const TICK_MS: u32 = 50;
const BOMB_FUSE_MS: u64 = 1500;
const EXPLOSION_MS: u64 = 400;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Pos {
    x: i32,
    y: i32,
}

#[derive(Clone, Copy)]
struct Bomb {
    pos: Pos,
    placed_at: Instant,
    exploded: bool,
}

struct GameState {
    player: Pos,
    bombs: Vec<Bomb>,
    explosions: Vec<(Pos, Instant)>,
}

impl GameState {
    fn new() -> Self {
        Self {
            player: Pos { x: 1, y: 1 },
            bombs: Vec::new(),
            explosions: Vec::new(),
        }
    }

    fn update(&mut self) {
        let now = Instant::now();

        // 폭탄 업데이트
        for bomb in &mut self.bombs {
            if !bomb.exploded
                && now.duration_since(bomb.placed_at).as_millis() as u64 >= BOMB_FUSE_MS
            {
                bomb.exploded = true;
                // 십자형 폭발 생성
                let center = bomb.pos;
                let dirs = [
                    Pos { x: 0, y: 0 },
                    Pos { x: 1, y: 0 },
                    Pos { x: -1, y: 0 },
                    Pos { x: 0, y: 1 },
                    Pos { x: 0, y: -1 },
                ];
                for d in dirs {
                    let p = Pos {
                        x: center.x + d.x,
                        y: center.y + d.y,
                    };
                    if p.x >= 0 && p.x < GRID_WIDTH && p.y >= 0 && p.y < GRID_HEIGHT {
                        self.explosions.push((p, now));
                    }
                }
            }
        }

        // 오래된 폭발 제거
        self.explosions
            .retain(|(_, t)| now.duration_since(*t).as_millis() as u64 <= EXPLOSION_MS);

        // 폭발이 끝난 폭탄 제거
        self.bombs.retain(|b| {
            now.duration_since(b.placed_at).as_millis() as u64 <= BOMB_FUSE_MS + EXPLOSION_MS
        });
    }

    fn move_player(&mut self, dx: i32, dy: i32) {
        let nx = self.player.x + dx;
        let ny = self.player.y + dy;
        if nx >= 0 && nx < GRID_WIDTH && ny >= 0 && ny < GRID_HEIGHT {
            self.player.x = nx;
            self.player.y = ny;
        }
    }

    fn place_bomb(&mut self) {
        // 해당 위치에 이미 폭탄이 있으면 무시
        if self
            .bombs
            .iter()
            .any(|b| b.pos.x == self.player.x && b.pos.y == self.player.y)
        {
            return;
        }

        self.bombs.push(Bomb {
            pos: self.player,
            placed_at: Instant::now(),
            exploded: false,
        });
    }
}

static GAME_STATE: OnceLock<Mutex<GameState>> = OnceLock::new();

fn game_state() -> &'static Mutex<GameState> {
    GAME_STATE.get_or_init(|| Mutex::new(GameState::new()))
}

unsafe fn create_brush(r: u8, g: u8, b: u8) -> HBRUSH {
    // COLORREF: 0x00bbggrr
    let color = COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16));
    windows::Win32::Graphics::Gdi::CreateSolidBrush(color)
}

unsafe fn draw_cell(hdc: HDC, grid_pos: Pos, brush: HBRUSH) {
    let left = grid_pos.x * CELL_SIZE;
    let top = grid_pos.y * CELL_SIZE;
    let right = left + CELL_SIZE;
    let bottom = top + CELL_SIZE;

    let old: HGDIOBJ = SelectObject(hdc, brush.into());
    let rect = RECT {
        left,
        top,
        right,
        bottom,
    };
    FillRect(hdc, &rect, brush);
    SelectObject(hdc, old);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);

    let bg_brush = create_brush(30, 144, 255); // 파란 배경
    let player_brush = create_brush(255, 255, 255); // 흰색 플레이어
    let bomb_brush = create_brush(0, 0, 0); // 검정 폭탄
    let explosion_brush = create_brush(255, 215, 0); // 노랑 폭발

    // 전체 배경
    let rect = RECT {
        left: 0,
        top: 0,
        right: GRID_WIDTH * CELL_SIZE,
        bottom: GRID_HEIGHT * CELL_SIZE,
    };
    FillRect(hdc, &rect, bg_brush);

    let state = game_state().lock().unwrap();

    // 폭탄
    for bomb in &state.bombs {
        draw_cell(hdc, bomb.pos, bomb_brush);
    }

    // 폭발
    for (pos, _) in &state.explosions {
        draw_cell(hdc, *pos, explosion_brush);
    }

    // 플레이어
    draw_cell(hdc, state.player, player_brush);

    let _ = DeleteObject(bg_brush.into());
    let _ = DeleteObject(player_brush.into());
    let _ = DeleteObject(bomb_brush.into());
    let _ = DeleteObject(explosion_brush.into());

    let _ = EndPaint(hwnd, &ps);
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let _ = game_state();
            SetTimer(Some(hwnd), TIMER_ID, TICK_MS, None);
        }
        WM_TIMER => {
            if wparam.0 == TIMER_ID {
                game_state().lock().unwrap().update();
                // 다시 그리기
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_KEYDOWN => {
            let mut state = game_state().lock().unwrap();
            match wparam.0 as u32 {
                0x25 => state.move_player(-1, 0), // VK_LEFT
                0x26 => state.move_player(0, -1), // VK_UP
                0x27 => state.move_player(1, 0),  // VK_RIGHT
                0x28 => state.move_player(0, 1),  // VK_DOWN
                0x20 => state.place_bomb(),       // VK_SPACE
                _ => {}
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_PAINT => {
            paint(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            PostQuitMessage(0);
        }
        _ => {}
    }

    DefWindowProcA(hwnd, msg, wparam, lparam)
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let hinstance = GetModuleHandleA(None)?;

        let class_name = PCSTR(b"BombGameWindowClass\0".as_ptr());

        let wc = WNDCLASSA {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };

        if RegisterClassA(&wc) == 0 {
            panic!("Failed to register window class");
        }

        let width = GRID_WIDTH * CELL_SIZE + 16;
        let height = GRID_HEIGHT * CELL_SIZE + 39;

        let hwnd = CreateWindowExA(
            Default::default(),
            class_name,
            PCSTR(b"BombGame\0".as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            None,
            None,
            Some(hinstance.into()),
            None,
        )?;

        if hwnd.0.is_null() {
            panic!("Failed to create window");
        }

        // 한글 제목은 유니코드 API로 설정해야 함 (PCSTR는 ASCII만 지원)
        let _ = SetWindowTextW(hwnd, w!("폭탄 게임"));

        let _ = ShowWindow(hwnd, SW_SHOW);

        // 메시지 루프
        let mut msg = MSG::default();
        while GetMessageA(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageA(&msg);
        }
    }

    Ok(())
}
