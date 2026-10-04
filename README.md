# GrannyQOLMod

A small native (Rust `cdylib`) mod for **Granny: Legacy** that removes several per-frame performance problems in the game's code, plus a hidden FPS counter.

## What it fixes

The game's IL2CPP code (`GameAssembly.dll`) does a lot of expensive lookups every single frame. The mod hooks the underlying Unity methods and makes them cheap:

| Problem in the game | Where | Fix |
|---|---|---|
| `GameObject.Find(...)` called unconditionally every frame | `Freezing`, `MagicalFreeze`, `GasExplosion`, `RagdollPL`, `BearTrapLogic` (3 per trap), `R_Granny` | Results are cached by name and reused while the object is still alive and active; otherwise the real `Find` runs. |
| `Shader.Find(...)` every frame while fading | `R_Granny`, `R_Grandpa` | Shaders cached by name. |
| `FindGameObjectsWithTag("BearTrapActivated")` (allocates a new array) every physics tick | `AI_Granny.FixedUpdate`, `AI_Grandpa.FixedUpdate` | Refreshed at most every 250 ms, only inside those two methods (they only read the array's length). |
| `PlayerPrefs.SetInt` + `PlayerPrefs.Save()` every frame (disk/registry flush) | `MainMenu.Update` | `Set*` calls that don't change the stored value are skipped, and `Save()` is skipped when nothing changed. |

All fixes are always on; there is no menu.

### FPS counter

Press **Shift + F9** to show/hide a minimal FPS counter in the top-left corner. It is hidden by default.

## How it works

- `DllMain` spawns a thread on attach so nothing heavy runs under the loader lock.
- The thread installs a DirectX 11 overlay via [hudhook](https://github.com/veeenu/hudhook) (FPS counter only), then waits for the IL2CPP runtime to come up.
- Target methods are resolved **by name** through IL2CPP's own exports (`il2cpp_class_from_name`, `il2cpp_class_get_method_from_name`), not hardcoded addresses, so minor game updates shouldn't break it. If any required method can't be found, nothing is patched.
- Hooks are installed with [retour](https://github.com/Hpmason/retour-rs) detours.
- Cached managed objects are held with IL2CPP GC handles so they can't be collected while cached, and Unity's native pointer (`m_CachedPtr`) is checked before reuse so destroyed objects are never returned.

## Building

Requires a Rust toolchain targeting `x86_64-pc-windows-msvc`.

```bash
cargo build --release
```

The output is `target/release/GrannyQOLMod.dll`.

## Installing

The game is 64-bit, so use 64-bit tools. Pick one of the options below.

### Option A: DLL loader (loads automatically every launch)

Use [Ultimate ASI Loader](https://github.com/ThirteenAG/Ultimate-ASI-Loader/releases) (x64):

1. Download the **x64** release and extract `dinput8.dll`.
2. Rename it to `winhttp.dll` and place it next to the game's `.exe` (the folder containing `GameAssembly.dll`).
3. Rename `GrannyQOLMod.dll` to `GrannyQOLMod.asi` and place it in the same folder (or in a `scripts` / `plugins` subfolder).
4. Launch the game normally.

The mod waits for the game to finish initializing, so loading early is fine.

### Option B: Injector (manual, per launch)

Any standard `LoadLibrary`-style injector works, e.g. [System Informer](https://systeminformer.sourceforge.io/) (right-click process → Miscellaneous → Inject DLL) or Xenos.

1. Start the game.
2. Inject `GrannyQOLMod.dll` into the game process.

Inject once per launch; injecting twice will try to install the hooks twice.

## Uninstalling

- Loader: delete `winhttp.dll` and `GrannyQOLMod.asi` from the game folder.
- Injector: just restart the game without injecting.

## Notes

- The mod does not change save data beyond skipping redundant `PlayerPrefs` writes; values are still saved whenever they actually change (and Unity flushes on quit).
- If two active objects share the same name, cached `GameObject.Find` keeps returning the one first found (Unity doesn't guarantee which one it returns anyway).
