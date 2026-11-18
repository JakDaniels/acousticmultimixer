// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! The plugin registers and describes itself.

#[test]
fn plugin_registers() {
    gst::init().unwrap();
    gstacousticmultimixer::plugin_register_static().unwrap();

    let plugin = gst::Registry::get()
        .find_plugin("acousticmultimixer")
        .expect("plugin registered");
    assert_eq!(plugin.license(), "MPL-2.0");
    assert_eq!(plugin.source(), "gst-plugin-acousticmultimixer");
}
