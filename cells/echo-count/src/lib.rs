use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// This import list is the cell's entire capability set; there is no WASI.
#[link(wasm_import_module = "rook")]
unsafe extern "C" {
    fn tick() -> i64;
    fn inbox_len() -> i32;
    fn inbox_meta(index: i32) -> i64;
    fn inbox_read(index: i32, pointer: i32, capacity: i32) -> i32;
    fn emit(channel_id: i32, pointer: i32, length: i32) -> i32;
    fn set_timer(tick: i64) -> i32;
    fn rand(pointer: i32, length: i32);
    fn param(key_pointer: i32, key_length: i32, output_pointer: i32, output_capacity: i32) -> i32;
    fn log(level: i32, pointer: i32, length: i32);
    fn abort(code: i32, pointer: i32, length: i32) -> !;
}
static COUNT: AtomicU64 = AtomicU64::new(0);
static ACTOR_ID: AtomicU32 = AtomicU32::new(0);
/// Initializes the stable actor identity supplied by the manifest.
#[unsafe(no_mangle)]
pub extern "C" fn rook_init(actor_id: u32) {
    ACTOR_ID.store(actor_id, Ordering::Relaxed);
}
/// Echoes each inbox payload with the cumulative count appended.
#[unsafe(no_mangle)]
pub extern "C" fn rook_step() {
    let message_count = unsafe { inbox_len() };
    let mut index = 0;
    while index < message_count {
        let mut payload = [0_u8; 16];
        let metadata = unsafe { inbox_meta(index) };
        let payload_length = metadata as u32;
        if payload_length as usize > payload.len() {
            unsafe { abort(1, payload.as_ptr() as i32, 0) };
        }
        unsafe { inbox_read(index, payload.as_mut_ptr() as i32, payload.len() as i32) };
        let count = COUNT.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let mut output = [0_u8; 24];
        output[..16].copy_from_slice(&payload);
        output[16..].copy_from_slice(&count.to_le_bytes());
        unsafe { emit(8, output.as_ptr() as i32, output.len() as i32) };
        index += 1;
    }
    let _ = unsafe { tick() };
    let _ = unsafe { set_timer(-1) };
    let mut random = [0_u8; 0];
    unsafe { rand(random.as_mut_ptr() as i32, 0) };
    let _ = unsafe { param(0, 0, 0, 0) };
    unsafe { log(0, 0, 0) };
}
