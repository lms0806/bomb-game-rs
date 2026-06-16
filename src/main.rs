use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::core::{w, PCSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DeleteObject, EndPaint, FillRect, InvalidateRect, SelectObject, HBRUSH, HDC,
    HGDIOBJ, PAINTSTRUCT,
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
// - Space 로 폭탄 설치 (기본 1개만 동시에 사용 가능)
// - 폭탄은 잠시 후 십자 모양으로 폭발
// - 랜덤 좌표에 몹이 나타나며, 폭발로 제거 가능
// - 몹 처치 시 10% 확률로 아이템 드랍 (초록: 범위 증가, 노랑: 폭탄 개수 증가)

const CELL_SIZE: i32 = 40;
const GRID_WIDTH: i32 = 13;
const GRID_HEIGHT: i32 = 11;

const TIMER_ID: usize = 1;
const TICK_MS: u32 = 50;
const BOMB_FUSE_MS: u64 = 1500;
const EXPLOSION_MS: u64 = 400;
const MAX_MOBS: usize = 8;
const MOB_SPAWN_INTERVAL_MS: u64 = 2500;
const INITIAL_MOB_COUNT: usize = 4;
const ITEM_DROP_CHANCE_PERCENT: u32 = 10;
const MAX_BOMB_RANGE: i32 = 5;
const MAX_BOMBS: usize = 5;
const DEFAULT_MAX_BOMBS: usize = 1;

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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Mob {
    pos: Pos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    RangeUp,
    BombCountUp,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Item {
    pos: Pos,
    kind: ItemKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PendingItemDrop {
    pos: Pos,
    kind: ItemKind,
}

struct GameState {
    player: Pos,
    bombs: Vec<Bomb>,
    explosions: Vec<(Pos, Instant)>,
    mobs: Vec<Mob>,
    items: Vec<Item>,
    pending_item_drops: Vec<PendingItemDrop>,
    bomb_range: i32,
    max_bombs: usize,
    rng_state: u64,
    last_spawn: Instant,
}

impl GameState {
    fn new() -> Self {
        let now = Instant::now();
        let mut state = Self {
            player: Pos { x: 1, y: 1 },
            bombs: Vec::new(),
            explosions: Vec::new(),
            mobs: Vec::new(),
            items: Vec::new(),
            pending_item_drops: Vec::new(),
            bomb_range: 1,
            max_bombs: DEFAULT_MAX_BOMBS,
            rng_state: now.elapsed().as_nanos() as u64 | 1,
            last_spawn: now,
        };

        for _ in 0..INITIAL_MOB_COUNT {
            state.spawn_mob();
        }

        state
    }

    fn next_random(&mut self) -> u32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        (self.rng_state >> 32) as u32
    }

    fn roll_percent(&mut self, percent: u32) -> bool {
        self.next_random() % 100 < percent
    }

    fn random_drop_kind(&mut self) -> ItemKind {
        if self.next_random() % 2 == 0 {
            ItemKind::RangeUp
        } else {
            ItemKind::BombCountUp
        }
    }

    fn is_occupied(&self, pos: Pos) -> bool {
        if self.player == pos {
            return true;
        }
        if self.mobs.iter().any(|m| m.pos == pos) {
            return true;
        }
        if self.bombs.iter().any(|b| b.pos == pos) {
            return true;
        }
        if self.items.iter().any(|i| i.pos == pos) {
            return true;
        }
        false
    }

    fn random_empty_pos(&mut self) -> Option<Pos> {
        let mut candidates = Vec::new();
        for y in 0..GRID_HEIGHT {
            for x in 0..GRID_WIDTH {
                let pos = Pos { x, y };
                if !self.is_occupied(pos) {
                    candidates.push(pos);
                }
            }
        }

        if candidates.is_empty() {
            return None;
        }

        let index = (self.next_random() as usize) % candidates.len();
        Some(candidates[index])
    }

    fn spawn_mob(&mut self) -> bool {
        if self.mobs.len() >= MAX_MOBS {
            return false;
        }

        let Some(pos) = self.random_empty_pos() else {
            return false;
        };

        self.mobs.push(Mob { pos });
        true
    }

    fn kill_mobs_in_explosions(&mut self) {
        let explosion_tiles: Vec<Pos> = self.explosions.iter().map(|(pos, _)| *pos).collect();

        let mut killed_mobs = Vec::new();
        self.mobs.retain(|mob| {
            let killed = explosion_tiles.iter().any(|pos| *pos == mob.pos);
            if killed {
                killed_mobs.push(*mob);
            }
            !killed
        });

        for mob in killed_mobs {
            if self.roll_percent(ITEM_DROP_CHANCE_PERCENT) {
                let kind = self.random_drop_kind();
                self.pending_item_drops.push(PendingItemDrop {
                    pos: mob.pos,
                    kind,
                });
            }
        }
    }

    fn process_pending_item_drops(&mut self) {
        let active_explosion_tiles: Vec<Pos> =
            self.explosions.iter().map(|(pos, _)| *pos).collect();

        self.pending_item_drops.retain(|drop| {
            if active_explosion_tiles.iter().any(|pos| *pos == drop.pos) {
                return true;
            }

            if !self.items.iter().any(|item| item.pos == drop.pos) {
                self.items.push(Item {
                    pos: drop.pos,
                    kind: drop.kind,
                });
            }
            false
        });
    }

    fn try_pickup_item(&mut self) {
        if let Some(index) = self.items.iter().position(|item| item.pos == self.player) {
            let item = self.items.remove(index);
            match item.kind {
                ItemKind::RangeUp => {
                    self.bomb_range = (self.bomb_range + 1).min(MAX_BOMB_RANGE);
                }
                ItemKind::BombCountUp => {
                    self.max_bombs = (self.max_bombs + 1).min(MAX_BOMBS);
                }
            }
        }
    }

    fn update(&mut self) {
        let now = Instant::now();

        // 폭탄 업데이트
        let exploding: Vec<Pos> = self
            .bombs
            .iter()
            .filter(|b| {
                !b.exploded && now.duration_since(b.placed_at).as_millis() as u64 >= BOMB_FUSE_MS
            })
            .map(|b| b.pos)
            .collect();

        for bomb in &mut self.bombs {
            if exploding.iter().any(|pos| *pos == bomb.pos) {
                bomb.exploded = true;
            }
        }

        for pos in exploding {
            self.add_cross_explosion(pos, now);
        }

        // 폭발 범위에 있는 몹 제거
        self.kill_mobs_in_explosions();

        // 오래된 폭발 제거
        self.explosions
            .retain(|(_, t)| now.duration_since(*t).as_millis() as u64 <= EXPLOSION_MS);

        // 폭발이 끝난 몹 사망 위치에 아이템 드랍
        self.process_pending_item_drops();

        // 폭발이 끝난 폭탄 제거
        self.bombs.retain(|b| {
            now.duration_since(b.placed_at).as_millis() as u64 <= BOMB_FUSE_MS + EXPLOSION_MS
        });

        // 주기적으로 몹 스폰
        if self.mobs.len() < MAX_MOBS
            && now.duration_since(self.last_spawn).as_millis() as u64 >= MOB_SPAWN_INTERVAL_MS
        {
            if self.spawn_mob() {
                self.last_spawn = now;
            }
        }
    }

    fn add_cross_explosion(&mut self, center: Pos, now: Instant) {
        if center.x >= 0 && center.x < GRID_WIDTH && center.y >= 0 && center.y < GRID_HEIGHT {
            self.explosions.push((center, now));
        }

        let directions = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        for (dx, dy) in directions {
            for dist in 1..=self.bomb_range {
                let p = Pos {
                    x: center.x + dx * dist,
                    y: center.y + dy * dist,
                };
                if p.x >= 0 && p.x < GRID_WIDTH && p.y >= 0 && p.y < GRID_HEIGHT {
                    self.explosions.push((p, now));
                }
            }
        }
    }

    fn move_player(&mut self, dx: i32, dy: i32) {
        let nx = self.player.x + dx;
        let ny = self.player.y + dy;
        let next = Pos { x: nx, y: ny };
        if nx >= 0
            && nx < GRID_WIDTH
            && ny >= 0
            && ny < GRID_HEIGHT
            && !self.mobs.iter().any(|m| m.pos == next)
        {
            self.player.x = nx;
            self.player.y = ny;
            self.try_pickup_item();
        }
    }

    fn place_bomb(&mut self) {
        // 동시에 놓을 수 있는 폭탄 수 제한
        if self.bombs.len() >= self.max_bombs {
            return;
        }

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
    let mob_brush = create_brush(220, 20, 60); // 빨간 몹
    let range_item_brush = create_brush(50, 205, 50); // 초록: 범위 증가
    let bomb_item_brush = create_brush(255, 255, 0); // 노랑: 폭탄 개수 증가
    let explosion_brush = create_brush(255, 165, 0); // 주황: 폭발

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

    // 몹
    for mob in &state.mobs {
        draw_cell(hdc, mob.pos, mob_brush);
    }

    // 폭발
    for (pos, _) in &state.explosions {
        draw_cell(hdc, *pos, explosion_brush);
    }

    // 아이템 (몹이 죽은 위치에 드랍)
    for item in &state.items {
        let brush = match item.kind {
            ItemKind::RangeUp => range_item_brush,
            ItemKind::BombCountUp => bomb_item_brush,
        };
        draw_cell(hdc, item.pos, brush);
    }

    // 플레이어
    draw_cell(hdc, state.player, player_brush);

    let _ = DeleteObject(bg_brush.into());
    let _ = DeleteObject(player_brush.into());
    let _ = DeleteObject(bomb_brush.into());
    let _ = DeleteObject(mob_brush.into());
    let _ = DeleteObject(range_item_brush.into());
    let _ = DeleteObject(bomb_item_brush.into());
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
