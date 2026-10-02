# Usage

## Launching

```sh
niri-display-manager
```

Or launch "Niri Display Manager" from Rofi, Fuzzel, or the application menu.
The application is single-instance: launching it again focuses the existing
window.

## Window overview

| Area | Meaning |
|---|---|
| Status | Current managed profile, mirror state, and an include warning if `display.kdl` is not registered in `config.kdl` |
| Display Profiles | The five profiles; the active one carries a check mark |
| Outputs | Every output niri reports, with mode, position, and scale |
| Hardware Ports | Every DRM connector from `/sys/class/drm` with `card` and cable state |
| Mirroring | Visible while mirroring: shows the state and a Stop Mirroring button |
| Header | Refresh button and a busy spinner while an apply is in progress |

Selecting a profile runs the full fail-safe pipeline. A toast reports the
result: either "Applied profile: ..." or a specific error; warnings (for
example "HDMI-A-1 has no cable attached") are shown as longer-lived toasts.

## Profiles

| Profile | Runtime effect | Persisted effect |
|---|---|---|
| Internal Display Only | Stops mirroring, restores the pristine configuration, re-enables `eDP-1` if needed | Removes the managed section; the file returns to its original bytes |
| Extend Right | External display placed at `x = internal logical width`, `y = 0` | Managed section with explicit positions |
| Extend Left | External display placed at negative `x` so it sits left of the panel | Managed section with explicit positions |
| Mirror / Presentation | Ensures both outputs are active, then spawns `wl-mirror --fullscreen-output <external> <internal>` | Managed section for the layout; wl-mirror runs as a supervised child |
| External Only | External display at `0,0`; the internal panel is turned off and focus moves to the external display | Managed section with `off` for the internal output |

Conditions:

- Extend with no cable attached still persists the layout and warns; niri
  applies it when the display is connected. Re-apply after connecting for
  exact position math on the external width (Extend Left).
- External Only and Mirror require a connected external display that niri has
  already adopted. This prevents black-screen states.
- Applying any profile stops a running mirror first, so mirror windows never
  outlive their profile.

## Presentation workflow (HDMI projector)

1. Connect the HDMI cable to the projector or TV.
2. Open the manager and press Refresh (or launch it).
3. Confirm the port appears as `connected` in Hardware Ports and the display
   appears in Outputs with a mode.
4. Select Mirror / Presentation. The projector receives a fullscreen mirror of
   the laptop panel.
5. Use Stop Mirroring, or select any other profile, to end the session.

If the projector shows nothing:

- Check that both `niri msg --json outputs` and the port list include the
  display; hotplug sometimes needs one Refresh.
- Verify `wl-mirror <internal> --fullscreen-output <external>` works in a
  terminal; the manager runs exactly this command.
- Some projectors need a moment to sync after the mode is set.

## Managing the backup

- `~/.config/niri/cfg/display.kdl.bak` is created once, before the first
  change, and is never overwritten by the manager.
- Internal Display Only restores this file exactly, then reloads niri.
- If the backup is missing when Internal Only is selected, the manager removes
  its managed section instead; if neither exists, the operation is a no-op.
- To return to fully manual configuration, select Internal Display Only and
  delete the `.bak` file.

## Command-line diagnostics

```sh
# Outputs as niri sees them (names, modes, scale, position)
niri msg --json outputs | jq

# Windows and workspaces (used to verify mirror placement)
niri msg --json windows | jq '.[] | {id, app_id, workspace_id}'
niri msg --json workspaces | jq '.[] | {id, output}'

# Cable state per port (ground truth for hotplug)
for status in /sys/class/drm/card*-*/status; do printf '%s: %s\n' "$status" "$(cat "$status")"; done

# Validate a hand-written config change before applying it
niri validate -c ~/.config/niri/config.kdl

# Verbose application logs
RUST_LOG=debug niri-display-manager
```

## Hybrid GPU diagnostics

The tested machine uses an Intel iGPU (`card1`) for the panel and exposes HDMI
ports on both GPUs:

```sh
niri msg --json outputs | jq -r 'keys[]'          # what niri manages
ls /sys/class/drm/                                 # all connectors
cat /sys/module/nvidia_drm/parameters/modeset      # must print Y on NVIDIA systems
journalctl -k -b | grep -iE 'drm|nvidia' | tail -30
```

Notes:

- The manager prefers HDMI ports on the same card as the panel when several
  external ports are available, then falls back to name order.
- If an NVIDIA-attached port stays `disconnected` at the driver level, kernel
  modesetting is usually disabled; set `nvidia_drm.modeset=1` and reboot.
- wl-mirror runs on Wayland and captures through niri's screencopy protocols;
  no portal configuration is required.

## Manual HDMI verification checklist

Run this when a physical display is attached (the automated tests cover the
cable-less paths):

1. `cat /sys/class/drm/card*-HDMI-*/status` shows `connected` for the port.
2. Refresh in the UI: the port turns `connected` and a new output row appears.
3. Extend Right: the external display sits to the right, cursor crosses the
   edge, and `niri msg --json outputs` reports `x == internal width`.
4. Extend Left: the external display sits to the left with a negative `x`.
5. Internal Display Only: the external output turns off, the file loses the
   managed section, and the panel keeps its normal layout.
6. External Only: the panel turns off, focus moves to the external display,
   and the file contains `off` for the internal output.
7. Mirror / Presentation: the external display shows a fullscreen copy of the
   panel; Stop Mirroring ends it and leaves both outputs enabled.
8. `pgrep -a wl-mirror` shows no process after stopping.
9. Reboot with an Extend profile active: niri restores the persisted layout.

## Troubleshooting

| Symptom | Explanation and fix |
|---|---|
| "no external display is connected" | The cable is not attached; connect it and press Refresh |
| "the external display is not available in niri yet" | The port is connected but niri has not adopted it; press Refresh, then retry |
| "niri rejected the generated configuration" | The file was automatically restored; report the message with your `display.kdl` content |
| "the mirror window did not appear" | wl-mirror exited immediately; run the printed command manually to see its error output |
| Mirror window is on the panel instead of the projector | The manager moves it to the target and re-verifies; if it persists, check that the external output is enabled and not fullscreen on another workspace |
| Profile applies but nothing changes | Check the include warning in the Status row; `config.kdl` must contain `include "./cfg/display.kdl"` |
| Active profile shows as "No managed profile applied" after edits | The `// profile:` comment in the managed section was removed; apply a profile again |
