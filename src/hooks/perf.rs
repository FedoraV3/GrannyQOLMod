//! Fixes for per-frame slowdowns found in GameAssembly.dll:
//! - GameObject.Find every frame (Freezing, MagicalFreeze, GasExplosion, RagdollPL, BearTrapLogic, R_Granny)
//! - Shader.Find during the R_Granny / R_Grandpa fade
//! - FindGameObjectsWithTag("BearTrapActivated") every AI_Granny / AI_Grandpa FixedUpdate
//! - PlayerPrefs.SetInt + PlayerPrefs.Save every frame in MainMenu

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use retour::RawDetour;

use crate::il2cpp::{self, GcHandle, Ptr};

const AI_TAG_TTL: Duration = Duration::from_millis(250);
const MAX_CACHE_ENTRIES: usize = 4096;

// trampolines to the original functions
static ORIG_GO_FIND: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHADER_FIND: AtomicUsize = AtomicUsize::new(0);
static ORIG_FIND_WITH_TAG: AtomicUsize = AtomicUsize::new(0);
static ORIG_GRANNY_FIXED: AtomicUsize = AtomicUsize::new(0);
static ORIG_GRANDPA_FIXED: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_INT: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_FLOAT: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_STRING: AtomicUsize = AtomicUsize::new(0);
static ORIG_DELETE_KEY: AtomicUsize = AtomicUsize::new(0);
static ORIG_DELETE_ALL: AtomicUsize = AtomicUsize::new(0);
static ORIG_SAVE: AtomicUsize = AtomicUsize::new(0);

// unhooked helpers we call ourselves
static ACTIVE_IN_HIERARCHY: AtomicUsize = AtomicUsize::new(0);
static GET_INT: AtomicUsize = AtomicUsize::new(0);
static GET_FLOAT: AtomicUsize = AtomicUsize::new(0);
static GET_STRING: AtomicUsize = AtomicUsize::new(0);
static HAS_KEY: AtomicUsize = AtomicUsize::new(0);

// unknown whether the game has unsaved prefs when we hook, so let the first Save through
static PREFS_DIRTY: AtomicBool = AtomicBool::new(true);

type StrToObj = unsafe extern "C" fn(Ptr, Ptr) -> Ptr;
type Method0 = unsafe extern "C" fn(Ptr, Ptr);
type Static0 = unsafe extern "C" fn(Ptr);
type ObjToBool = unsafe extern "C" fn(Ptr, Ptr) -> bool;
type SetInt = unsafe extern "C" fn(Ptr, i32, Ptr);
type GetInt = unsafe extern "C" fn(Ptr, i32, Ptr) -> i32;
type SetFloat = unsafe extern "C" fn(Ptr, f32, Ptr);
type GetFloat = unsafe extern "C" fn(Ptr, f32, Ptr) -> f32;
type SetString = unsafe extern "C" fn(Ptr, Ptr, Ptr);
type GetString = unsafe extern "C" fn(Ptr, Ptr, Ptr) -> Ptr;

unsafe fn func<F: Copy>(slot: &AtomicUsize) -> F {
    let addr = slot.load(Relaxed);
    unsafe { std::mem::transmute_copy(&addr) }
}

// Unity APIs below are main-thread only, so per-thread caches are effectively global.
thread_local! {
    static FIND_MAP: RefCell<HashMap<Vec<u16>, GcHandle>> = RefCell::new(HashMap::new());
    static SHADER_MAP: RefCell<HashMap<Vec<u16>, GcHandle>> = RefCell::new(HashMap::new());
    static TAG_MAP: RefCell<HashMap<Vec<u16>, (GcHandle, Instant)>> = RefCell::new(HashMap::new());
    static AI_DEPTH: Cell<u32> = const { Cell::new(0) };
}

fn store<V>(map: &RefCell<HashMap<Vec<u16>, V>>, key: Vec<u16>, value: Option<V>) {
    let mut map = map.borrow_mut();
    match value {
        Some(value) => {
            if map.len() >= MAX_CACHE_ENTRIES && !map.contains_key(&key) {
                map.clear();
            }
            map.insert(key, value);
        }
        None => {
            map.remove(&key);
        }
    }
}

unsafe extern "C" fn hk_gameobject_find(name: Ptr, method: Ptr) -> Ptr {
    let orig: StrToObj = unsafe { func(&ORIG_GO_FIND) };
    if name.is_null() {
        return unsafe { orig(name, method) };
    }
    let key = unsafe { il2cpp::string_key(name) };
    let cached = FIND_MAP.with(|m| m.borrow().get(&key).map(GcHandle::target));
    if let Some(obj) = cached {
        // Find only returns active objects, so a deactivated hit must fall through
        let active: ObjToBool = unsafe { func(&ACTIVE_IN_HIERARCHY) };
        if unsafe { il2cpp::unity_alive(obj) && active(obj, std::ptr::null_mut()) } {
            return obj;
        }
    }
    let obj = unsafe { orig(name, method) };
    FIND_MAP.with(|m| store(m, key, (!obj.is_null()).then(|| GcHandle::new(obj))));
    obj
}

unsafe extern "C" fn hk_shader_find(name: Ptr, method: Ptr) -> Ptr {
    let orig: StrToObj = unsafe { func(&ORIG_SHADER_FIND) };
    if name.is_null() {
        return unsafe { orig(name, method) };
    }
    let key = unsafe { il2cpp::string_key(name) };
    let cached = SHADER_MAP.with(|m| m.borrow().get(&key).map(GcHandle::target));
    if let Some(shader) = cached {
        if unsafe { il2cpp::unity_alive(shader) } {
            return shader;
        }
    }
    let shader = unsafe { orig(name, method) };
    SHADER_MAP.with(|m| store(m, key, (!shader.is_null()).then(|| GcHandle::new(shader))));
    shader
}

/// Only throttled inside the AI FixedUpdates, which just read `traps.Length`.
unsafe extern "C" fn hk_find_gameobjects_with_tag(tag: Ptr, method: Ptr) -> Ptr {
    let orig: StrToObj = unsafe { func(&ORIG_FIND_WITH_TAG) };
    if AI_DEPTH.get() == 0 || tag.is_null() {
        return unsafe { orig(tag, method) };
    }
    let key = unsafe { il2cpp::string_key(tag) };
    let cached = TAG_MAP.with(|m| {
        m.borrow()
            .get(&key)
            .filter(|(_, at)| at.elapsed() < AI_TAG_TTL)
            .map(|(h, _)| h.target())
    });
    if let Some(array) = cached.filter(|a| !a.is_null()) {
        return array;
    }
    let array = unsafe { orig(tag, method) };
    TAG_MAP.with(|m| store(m, key, (!array.is_null()).then(|| (GcHandle::new(array), Instant::now()))));
    array
}

unsafe fn in_ai(slot: &AtomicUsize, this: Ptr, method: Ptr) {
    let orig: Method0 = unsafe { func(slot) };
    AI_DEPTH.set(AI_DEPTH.get() + 1);
    unsafe { orig(this, method) };
    AI_DEPTH.set(AI_DEPTH.get() - 1);
}

unsafe extern "C" fn hk_granny_fixed_update(this: Ptr, method: Ptr) {
    unsafe { in_ai(&ORIG_GRANNY_FIXED, this, method) }
}

unsafe extern "C" fn hk_grandpa_fixed_update(this: Ptr, method: Ptr) {
    unsafe { in_ai(&ORIG_GRANDPA_FIXED, this, method) }
}

unsafe extern "C" fn hk_set_int(key: Ptr, value: i32, method: Ptr) {
    if !key.is_null() {
        let get: GetInt = unsafe { func(&GET_INT) };
        // a default that differs from `value` means a match proves the key exists
        if unsafe { get(key, value ^ 1, std::ptr::null_mut()) } == value {
            return;
        }
    }
    let orig: SetInt = unsafe { func(&ORIG_SET_INT) };
    unsafe { orig(key, value, method) };
    PREFS_DIRTY.store(true, Relaxed);
}

unsafe extern "C" fn hk_set_float(key: Ptr, value: f32, method: Ptr) {
    if !key.is_null() {
        let get: GetFloat = unsafe { func(&GET_FLOAT) };
        let fallback = if value == 1.0 { 2.0 } else { 1.0 };
        if unsafe { get(key, fallback, std::ptr::null_mut()) }.to_bits() == value.to_bits() {
            return;
        }
    }
    let orig: SetFloat = unsafe { func(&ORIG_SET_FLOAT) };
    unsafe { orig(key, value, method) };
    PREFS_DIRTY.store(true, Relaxed);
}

unsafe extern "C" fn hk_set_string(key: Ptr, value: Ptr, method: Ptr) {
    if !key.is_null() && !value.is_null() {
        let has: ObjToBool = unsafe { func(&HAS_KEY) };
        let get: GetString = unsafe { func(&GET_STRING) };
        if unsafe { has(key, std::ptr::null_mut()) } {
            let current = unsafe { get(key, value, std::ptr::null_mut()) };
            if !current.is_null() && unsafe { il2cpp::string_key(current) == il2cpp::string_key(value) } {
                return;
            }
        }
    }
    let orig: SetString = unsafe { func(&ORIG_SET_STRING) };
    unsafe { orig(key, value, method) };
    PREFS_DIRTY.store(true, Relaxed);
}

unsafe extern "C" fn hk_delete_key(key: Ptr, method: Ptr) {
    let orig: Method0 = unsafe { func(&ORIG_DELETE_KEY) };
    unsafe { orig(key, method) };
    PREFS_DIRTY.store(true, Relaxed);
}

unsafe extern "C" fn hk_delete_all(method: Ptr) {
    let orig: Static0 = unsafe { func(&ORIG_DELETE_ALL) };
    unsafe { orig(method) };
    PREFS_DIRTY.store(true, Relaxed);
}

unsafe extern "C" fn hk_save(method: Ptr) {
    if !PREFS_DIRTY.load(Relaxed) {
        return;
    }
    PREFS_DIRTY.store(false, Relaxed);
    let orig: Static0 = unsafe { func(&ORIG_SAVE) };
    unsafe { orig(method) };
}

struct Hook {
    ns: &'static str,
    class: &'static str,
    method: &'static str,
    argc: i32,
    detour: *const (),
    orig: &'static AtomicUsize,
    // IL2CPP strips methods the game never calls; those don't need hooking
    optional: bool,
}

pub fn install() -> Result<()> {
    il2cpp::wait_for_runtime(Duration::from_secs(120))?;
    let _attached = il2cpp::ThreadGuard::attach();

    for (class, method, argc, slot) in [
        ("GameObject", "get_activeInHierarchy", 0, &ACTIVE_IN_HIERARCHY),
        ("PlayerPrefs", "GetInt", 2, &GET_INT),
        ("PlayerPrefs", "GetFloat", 2, &GET_FLOAT),
        ("PlayerPrefs", "GetString", 2, &GET_STRING),
        ("PlayerPrefs", "HasKey", 1, &HAS_KEY),
    ] {
        slot.store(il2cpp::method_pointer("UnityEngine", class, method, argc)?, Relaxed);
    }

    let hook = |ns, class, method, argc, detour: *const (), orig| Hook { ns, class, method, argc, detour, orig, optional: false };
    let optional = |h: Hook| Hook { optional: true, ..h };
    let hooks = [
        hook("UnityEngine", "GameObject", "Find", 1, hk_gameobject_find as *const (), &ORIG_GO_FIND),
        hook("UnityEngine", "Shader", "Find", 1, hk_shader_find as *const (), &ORIG_SHADER_FIND),
        hook("UnityEngine", "GameObject", "FindGameObjectsWithTag", 1, hk_find_gameobjects_with_tag as *const (), &ORIG_FIND_WITH_TAG),
        hook("", "AI_Granny", "FixedUpdate", 0, hk_granny_fixed_update as *const (), &ORIG_GRANNY_FIXED),
        hook("", "AI_Grandpa", "FixedUpdate", 0, hk_grandpa_fixed_update as *const (), &ORIG_GRANDPA_FIXED),
        hook("UnityEngine", "PlayerPrefs", "SetInt", 2, hk_set_int as *const (), &ORIG_SET_INT),
        hook("UnityEngine", "PlayerPrefs", "SetFloat", 2, hk_set_float as *const (), &ORIG_SET_FLOAT),
        hook("UnityEngine", "PlayerPrefs", "SetString", 2, hk_set_string as *const (), &ORIG_SET_STRING),
        optional(hook("UnityEngine", "PlayerPrefs", "DeleteKey", 1, hk_delete_key as *const (), &ORIG_DELETE_KEY)),
        optional(hook("UnityEngine", "PlayerPrefs", "DeleteAll", 0, hk_delete_all as *const (), &ORIG_DELETE_ALL)),
        hook("UnityEngine", "PlayerPrefs", "Save", 0, hk_save as *const (), &ORIG_SAVE),
    ];

    // resolve everything before patching anything, and refuse identical-code-folded targets
    let mut targets = Vec::with_capacity(hooks.len());
    for h in &hooks {
        let target = match il2cpp::method_pointer(h.ns, h.class, h.method, h.argc) {
            Ok(target) => target,
            Err(_) if h.optional => continue,
            Err(e) => return Err(e),
        };
        if targets.iter().any(|(_, t)| *t == target) {
            return Err(anyhow!("{}::{} shares code with another hooked method", h.class, h.method));
        }
        targets.push((h, target));
    }

    for (h, target) in targets {
        let detour = unsafe { RawDetour::new(target as *const (), h.detour) }
            .map_err(|e| anyhow!("detour {}::{}: {e}", h.class, h.method))?;
        h.orig.store(detour.trampoline() as *const () as usize, Relaxed);
        unsafe { detour.enable() }.map_err(|e| anyhow!("enable {}::{}: {e}", h.class, h.method))?;
        // keep the patch alive for the life of the process
        Box::leak(Box::new(detour));
    }
    Ok(())
}
