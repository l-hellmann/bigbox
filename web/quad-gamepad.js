// quad-gamepad.js — bridge the browser Gamepad API to the wasm as plain numbers.
//
// A miniquad plugin (same mechanism as quad-url.js): registers `quad_gamepad_*`
// env imports the wasm calls each frame. Axes/buttons are numbers, so nothing
// needs marshaling — no wasm-bindgen, no sapp-jsutils. Load after the macroquad
// bundle and before load().
//
// The Gamepad API only surfaces a controller after the user presses a button on
// it (a privacy gate), so `connected` returns 0 until then — expected.

var quad_gamepad_current = null;

function quad_gamepad_register_js_plugin(importObject) {
    // Refresh the cached gamepad snapshot and report whether one is connected.
    // Called once per poll before the axis/button reads, so they all see a
    // consistent snapshot.
    importObject.env.quad_gamepad_connected = function () {
        var pads = navigator.getGamepads ? navigator.getGamepads() : [];
        quad_gamepad_current = null;
        for (var i = 0; i < pads.length; i++) {
            if (pads[i] && pads[i].connected) {
                quad_gamepad_current = pads[i];
                break;
            }
        }
        return quad_gamepad_current ? 1 : 0;
    };

    importObject.env.quad_gamepad_axis = function (index) {
        var p = quad_gamepad_current;
        if (p == null || index < 0 || index >= p.axes.length) return 0.0;
        return p.axes[index];
    };

    importObject.env.quad_gamepad_button = function (index) {
        var p = quad_gamepad_current;
        if (p == null || index < 0 || index >= p.buttons.length) return 0.0;
        return p.buttons[index].value;
    };
}

miniquad_add_plugin({
    register_plugin: quad_gamepad_register_js_plugin,
    name: "quad_gamepad",
    version: 1,
});
