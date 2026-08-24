// Copyright 2026 Josh Heinrichs
// SPDX-License-Identifier: GPL-2.0-or-later
//
// The pad reports every key and encoder event to the host over raw HID and
// holds no policy of its own: which mode is active, what a knob does and what
// colour the LEDs are all live in the host daemon. All this firmware owns is a
// 27-entry LED buffer the host paints into.

#include QMK_KEYBOARD_H
#include "raw_hid.h"
#include <string.h>

enum layer_names { _BASE, _BOOT };

// raw_hid_send() drops anything that isn't exactly this long (see RAW_EPSIZE in
// tmk_core/protocol/usb_descriptor.h, which raw_hid.h does not re-export).
#define RAW_MSG_SIZE 32

// Pad -> host
#define EVT_KEY 0x01
#define EVT_ENCODER 0x02

// Host -> pad
#define CMD_PING 0x04
#define CMD_LEDS 0x02


// What the pad falls back to once the host goes quiet, so a dead daemon looks
// obviously different from a working one rather than merely unresponsive.
#define HOST_TIMEOUT_MS 2000
#define IDLE_LEVEL 8

static uint8_t  led_buf[RGB_MATRIX_LED_COUNT][3];
static uint32_t last_host_msg;
static bool     host_seen;

// clang-format off
const uint16_t PROGMEM keymaps[][MATRIX_ROWS][MATRIX_COLS] = {
    // Nothing here emits a keycode -- every press is reported to the host
    // instead. The top-left key doubles as the hold for the bootloader layer.
    [_BASE] = LAYOUT_top(
                    KC_NO,   KC_NO,   KC_NO,
        MO(_BOOT), KC_NO,   KC_NO,   KC_NO,   KC_NO,
        KC_NO,     KC_NO,   KC_NO,   KC_NO,   KC_NO,
        KC_NO,     KC_NO,   KC_NO,   KC_NO,   KC_NO
    ),

    // Hold top-left, press bottom-right to reflash without unplugging.
    [_BOOT] = LAYOUT_top(
                    KC_TRNS, KC_TRNS, KC_TRNS,
        KC_TRNS,   KC_TRNS, KC_TRNS, KC_TRNS, KC_TRNS,
        KC_TRNS,   KC_TRNS, KC_TRNS, KC_TRNS, KC_TRNS,
        KC_TRNS,   KC_TRNS, KC_TRNS, KC_TRNS, QK_BOOT
    ),
};

// Matrix columns 0..14 are the 15 keys in reading order, and 15/16/17 are the
// centre/right/left encoder switches. Report the switches as 15/16/17 in
// left/centre/right order so they line up with the encoder indices themselves.
static uint8_t key_index(uint8_t col) {
    switch (col) {
        case 17:
            return 15;
        case 15:
            return 16;
        case 16:
            return 17;
        default:
            return col;
    }
}

void raw_hid_receive(uint8_t *data, uint8_t length) {
    if (length < 1) return;
    last_host_msg = timer_read32();
    host_seen     = true;

    switch (data[0]) {
        // Every LED in one report, one byte each as RGB332. Three bytes per LED
        // would need 81 and the report holds 32; at a byte each all 27 fit with
        // room to spare, so the host can paint anything it likes without a new
        // opcode per effect. Colour depth is the price: 8 reds, 8 greens,
        // 4 blues.
        // A run of LEDs at full depth, three bytes each. Nine fit in a report,
        // so the whole pad is three of them. Packed formats were tried and are
        // not worth it: the host then has to reason about which colours are
        // representable, and greys stop being grey.
        case CMD_LEDS: {
            uint8_t offset = data[1];
            uint8_t count  = data[2];
            if (count > (RAW_MSG_SIZE - 3) / 3) count = (RAW_MSG_SIZE - 3) / 3;
            for (uint8_t i = 0; i < count && (offset + i) < RGB_MATRIX_LED_COUNT; i++) {
                led_buf[offset + i][0] = data[3 + i * 3];
                led_buf[offset + i][1] = data[4 + i * 3];
                led_buf[offset + i][2] = data[5 + i * 3];
            }
            break;
        }

        case CMD_PING:
            break;
    }
}

bool process_record_user(uint16_t keycode, keyrecord_t *record) {
    uint8_t msg[RAW_MSG_SIZE] = {0};
    msg[0]                  = EVT_KEY;
    msg[1]                  = key_index(record->event.key.col);
    msg[2]                  = record->event.pressed ? 1 : 0;
    raw_hid_send(msg, sizeof(msg));

    // The layer hold and the bootloader key are the only two that still need
    // QMK to act on them locally.
    return keycode == MO(_BOOT) || keycode == QK_BOOT;
}

bool encoder_update_user(uint8_t index, bool clockwise) {
    uint8_t msg[RAW_MSG_SIZE] = {0};
    msg[0]                  = EVT_ENCODER;
    msg[1]                  = index;
    msg[2]                  = clockwise ? 1 : (uint8_t)-1;
    raw_hid_send(msg, sizeof(msg));

    // Suppress the board's built-in media keys.
    return false;
}

bool rgb_matrix_indicators_user(void) {
    bool live = host_seen && timer_elapsed32(last_host_msg) < HOST_TIMEOUT_MS;
    for (uint8_t i = 0; i < RGB_MATRIX_LED_COUNT; i++) {
        if (live) {
            rgb_matrix_set_color(i, led_buf[i][0], led_buf[i][1], led_buf[i][2]);
        } else {
            rgb_matrix_set_color(i, IDLE_LEVEL, IDLE_LEVEL, IDLE_LEVEL);
        }
    }
    return false;
}
