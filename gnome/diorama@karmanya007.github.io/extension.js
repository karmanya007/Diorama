// Diorama for GNOME Shell: puts Diorama's wallpaper windows behind the
// desktop.
//
// GNOME's compositor has no layer-shell, so dioramad opens one ordinary
// window per monitor with the app id below and the title
// `Diorama wallpaper|<connector>`. This extension keeps each one on its
// monitor, below every other window, on every workspace, unfocusable, and
// out of the overview's windows, the window switcher and the dash. The
// overview's workspaces show it as their background.
//
// GNOME does not tell applications about other windows, so dioramad asks
// this extension what covers each monitor (`Coverage` on GNOME Shell's bus
// name, object path below) to pause wallpapers behind fullscreen and
// maximized windows.

import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as WorkspaceUi from 'resource:///org/gnome/shell/ui/workspace.js';
import {WorkspaceThumbnail} from 'resource:///org/gnome/shell/ui/workspaceThumbnail.js';
import {Extension, InjectionManager} from 'resource:///org/gnome/shell/extensions/extension.js';

const APP_ID = 'io.github.karmanya007.diorama.wallpaper';
const TITLE_PREFIX = 'Diorama wallpaper|';
const DBUS_PATH = '/io/github/karmanya007/Diorama';
const DBUS_INTERFACE = `
<node>
  <interface name="io.github.karmanya007.Diorama1">
    <!-- Per wallpaper connector: "fullscreen", "maximized" or "visible". -->
    <method name="Coverage">
      <arg type="a{ss}" direction="out" name="coverage"/>
    </method>
  </interface>
</node>`;

/** Whether `window` (a window, or its actor) is one of Diorama's wallpapers. */
function isWallpaper(window) {
    const meta = window?.get_meta_window?.() ?? window;
    return meta?.get_wm_class?.() === APP_ID;
}

/** Cuts an actor to the rounded rectangle GNOME cuts workspace backgrounds to. */
const RoundedClip = GObject.registerClass(
class DioramaRoundedClip extends Shell.GLSLEffect {
    vfunc_build_pipeline() {
        // GNOME 48 moved the hooks.
        const hook = (Shell.SnippetHook ?? Cogl.SnippetHook).FRAGMENT;
        this.add_glsl_snippet(hook, `
            uniform vec2 actor_size;
            uniform vec4 bounds; // x, y, width, height
            uniform float radius;`, `
            vec2 half_size = bounds.zw / 2.0;
            vec2 p = abs(cogl_tex_coord_in[0].xy * actor_size - bounds.xy - half_size);
            vec2 d = p - half_size + radius;
            float outside = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
            cogl_color_out *= clamp(0.5 - outside, 0.0, 1.0);`, false);
    }

    /** All in the monitor's pixels, whatever size the actor is shown at. */
    update(monitor, workarea, radius) {
        const set = (name, values) =>
            this.set_uniform_float(this.get_uniform_location(name), values.length, values);
        set('actor_size', [monitor.width, monitor.height]);
        set('bounds', [workarea.x - monitor.x, workarea.y - monitor.y, workarea.width, workarea.height]);
        set('radius', [radius]);
        this.queue_repaint();
    }
});

export default class DioramaExtension extends Extension {
    enable() {
        this._windows = new Map();
        this._clones = new Set();
        this._injections = new InjectionManager();
        const display = global.display;

        this._signals = [
            [display, display.connect('window-created', (_d, w) => this._track(w))],
            [display, display.connect('restacked', () => this._lowerAll())],
            [display, display.connect('notify::focus-window', () => this._refuseFocus())],
            [Main.layoutManager, Main.layoutManager.connect('monitors-changed', () => this._placeAll())],
        ];
        for (const actor of global.get_window_actors())
            this._track(actor.get_meta_window());

        // Out of the overview and its workspace thumbnails.
        for (const proto of [WorkspaceUi.Workspace.prototype, WorkspaceThumbnail.prototype]) {
            this._injections.overrideMethod(proto, '_isOverviewWindow', original =>
                function (window) {
                    return !isWallpaper(window) && original.call(this, window);
                });
        }
        // Out of Alt+Tab and the other window lists.
        this._injections.overrideMethod(Meta.Display.prototype, 'get_tab_list', original =>
            function (...args) {
                return original.apply(this, args).filter(w => !isWallpaper(w));
            });
        // Out of the dash's running apps.
        this._injections.overrideMethod(Shell.AppSystem.prototype, 'get_running', original =>
            function (...args) {
                return original.apply(this, args).filter(app =>
                    !app.get_windows().some(isWallpaper));
            });

        this._showInOverview();

        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_INTERFACE, this);
        this._dbus.export(Gio.DBus.session, DBUS_PATH);
    }

    disable() {
        this._dbus.unexport();
        this._dbus = null;
        this._injections.clear();
        this._injections = null;
        for (const clone of this._clones)
            clone.destroy();
        this._clones = null;
        for (const [object, id] of this._signals)
            object.disconnect(id);
        this._signals = null;
        for (const [window, ids] of this._windows) {
            for (const id of ids)
                window.disconnect(id);
        }
        this._windows = null;
    }

    /** The overview draws GNOME's own picture behind each workspace: cover it. */
    _showInOverview() {
        const proto = WorkspaceUi.WorkspaceBackground?.prototype;
        if (!proto?._updateBorderRadius || !proto._updateRoundedClipBounds)
            return;
        const extension = this;
        this._injections.overrideMethod(proto, '_init', original =>
            function (...args) {
                original.apply(this, args);
                try {
                    extension._coverBackground(this);
                } catch (e) {
                    logError(e, 'Diorama: overview background');
                }
            });
        for (const name of ['_updateBorderRadius', '_updateRoundedClipBounds']) {
            this._injections.overrideMethod(proto, name, original =>
                function (...args) {
                    original.apply(this, args);
                    extension._clipClone(this);
                });
        }
    }

    _coverBackground(background) {
        // The newest, while a wallpaper replaces another.
        const actor = [...this._windows.keys()]
            .filter(w => w.get_monitor() === background._monitorIndex)
            .pop()?.get_compositor_private();
        if (!actor || !background._backgroundGroup || !background._bgManager)
            return;
        const clone = new Clutter.Clone({source: actor, x_expand: true, y_expand: true});
        clone.add_effect_with_name('clip', new RoundedClip());
        background._backgroundGroup.add_child(clone);
        // GNOME's picture may be replaced by a new one on top.
        background._bgManager.connect('changed', () =>
            clone.get_parent()?.set_child_above_sibling(clone, null));
        background._dioramaClone = clone;
        this._clones.add(clone);
        clone.connect('destroy', () => {
            background._dioramaClone = null;
            this._clones?.delete(clone);
        });
        this._clipClone(background);
    }

    _clipClone(background) {
        const monitor = Main.layoutManager.monitors[background._monitorIndex];
        if (!monitor)
            return;
        background._dioramaClone?.get_effect('clip').update(monitor, background._workarea,
            background._bgManager.backgroundActor.content.rounded_clip_radius);
    }

    _track(window) {
        if (!window || this._windows.has(window))
            return;
        if (!isWallpaper(window)) {
            // A Wayland window's app id arrives with its first commit.
            if (window.get_wm_class() === null) {
                const id = window.connect('notify::wm-class', () => {
                    window.disconnect(id);
                    this._track(window);
                });
            }
            return;
        }
        const ids = [
            window.connect('unmanaged', () => this._windows?.delete(window)),
            window.connect('notify::title', () => this._place(window)),
            window.connect('position-changed', () => this._place(window)),
            window.connect('size-changed', () => this._place(window)),
        ];
        this._windows.set(window, ids);
        window.stick();
        this._place(window);
        window.lower();
        // The window gets its actor, and mutter its final position, later.
        const actor = window.get_compositor_private();
        if (actor) {
            const id = actor.connect('first-frame', () => {
                actor.disconnect(id);
                this._placeSoon(window);
            });
        }
        this._placeSoon(window);
    }

    /** Places `window` once mutter has applied its constraints. */
    _placeSoon(window) {
        GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            if (this._windows?.has(window))
                this._place(window);
            return GLib.SOURCE_REMOVE;
        });
    }

    /** The monitor the title names, else the window's own. */
    _monitor(window) {
        const title = window.get_title() ?? '';
        if (title.startsWith(TITLE_PREFIX)) {
            const connector = title.slice(TITLE_PREFIX.length);
            const index = global.backend.get_monitor_manager().get_monitor_for_connector(connector);
            if (index >= 0)
                return Main.layoutManager.monitors[index];
        }
        return Main.layoutManager.monitors[window.get_monitor()];
    }

    _place(window) {
        const monitor = this._monitor(window);
        if (!monitor)
            return;
        const frame = window.get_frame_rect();
        const elsewhere = frame.x < monitor.x || frame.x >= monitor.x + monitor.width ||
            frame.y < monitor.y || frame.y >= monitor.y + monitor.height;
        if (elsewhere || frame.width !== monitor.width || frame.height !== monitor.height) {
            // Placed for real when mutter answers (position-changed).
            window.move_resize_frame(false, monitor.x, monitor.y, monitor.width, monitor.height);
            return;
        }
        // GNOME keeps windows out of the top bar's strip; the wallpaper
        // belongs under it too, so its picture (and input) is shifted there.
        window.get_compositor_private()?.set_translation(
            monitor.x - frame.x, monitor.y - frame.y, 0);
    }

    _placeAll() {
        for (const window of this._windows.keys())
            this._place(window);
    }

    /** Below every other window (GNOME raises windows it focuses). */
    _lowerAll() {
        for (const window of this._windows.keys())
            window.lower();
    }

    /** D-Bus: what covers each wallpaper's monitor on the active workspace. */
    Coverage() {
        const workspace = global.workspace_manager.get_active_workspace();
        const others = global.display.get_tab_list(Meta.TabList.NORMAL, workspace)
            .filter(w => !w.minimized);
        const coverage = {};
        for (const window of this._windows.keys()) {
            const title = window.get_title() ?? '';
            if (!title.startsWith(TITLE_PREFIX))
                continue;
            const monitor = window.get_monitor();
            const here = others.filter(w => w.get_monitor() === monitor);
            coverage[title.slice(TITLE_PREFIX.length)] =
                here.some(w => w.is_fullscreen()) ? 'fullscreen'
                    : here.some(w => w.get_maximized() === Meta.MaximizeFlags.BOTH) ? 'maximized'
                        : 'visible';
        }
        return coverage;
    }

    /** The desktop never takes the keyboard: hand focus back. */
    _refuseFocus() {
        const focused = global.display.focus_window;
        if (!isWallpaper(focused))
            return;
        const time = global.get_current_time();
        const next = global.display.get_tab_list(Meta.TabList.NORMAL, null)[0];
        if (next)
            next.activate(time);
        else
            global.display.unset_input_focus(time);
        this._lowerAll();
    }
}
