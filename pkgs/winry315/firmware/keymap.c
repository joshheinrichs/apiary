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
#define CMD_SOLID 0x01
#define CMD_KEYS 0x02
#define CMD_BANDS 0x03
#define CMD_PING 0x04
#define CMD_FRAME 0x05

// Overrides that fit after the opcode and wash colour, at 4 bytes each.
#define FRAME_MAX_OVERRIDES ((RAW_MSG_SIZE - 5) / 4)

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

// Each column of the key grid, bottom LED first, for the bar-graph rendering
// CMD_BANDS does. Indices come from the LED map in winry315.c.
static const uint8_t PROGMEM band_leds[5][3] = {
    { 8,  7,  6},
    { 9, 10, 11},
    {14, 13, 12},
    {15, 16, 17},
    {20, 19, 18},
};
// clang-format on

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

static void render_bands(const uint8_t *rgb, uint8_t count, const uint8_t *levels) {
    memset(led_buf, 0, sizeof(led_buf));
    if (count > 5) count = 5;
    for (uint8_t c = 0; c < count; c++) {
        // Spread one 0..255 level across the column's three LEDs, so a level of
        // 255 lights all three and anything less part-fills from the bottom.
        uint16_t scaled = (uint16_t)levels[c] * 3;
        for (uint8_t r = 0; r < 3; r++) {
            uint16_t seg = scaled > (uint16_t)r * 255 ? scaled - (uint16_t)r * 255 : 0;
            if (seg > 255) seg = 255;
            uint8_t led      = pgm_read_byte(&band_leds[c][r]);
            led_buf[led][0] = (uint8_t)((uint16_t)rgb[0] * seg / 255);
            led_buf[led][1] = (uint8_t)((uint16_t)rgb[1] * seg / 255);
            led_buf[led][2] = (uint8_t)((uint16_t)rgb[2] * seg / 255);
        }
    }
}

void raw_hid_receive(uint8_t *data, uint8_t length) {
    if (length < 1) return;
    last_host_msg = timer_read32();
    host_seen     = true;

    switch (data[0]) {
        case CMD_SOLID:
            for (uint8_t i = 0; i < RGB_MATRIX_LED_COUNT; i++) {
                led_buf[i][0] = data[1];
                led_buf[i][1] = data[2];
                led_buf[i][2] = data[3];
            }
            break;

        case CMD_KEYS: {
            uint8_t offset = data[1];
            uint8_t count  = data[2];
            // Three header bytes leave room for nine triples in a 32-byte
            // report; anything beyond that would read past the buffer.
            if (count > (RAW_MSG_SIZE - 3) / 3) count = (RAW_MSG_SIZE - 3) / 3;
            for (uint8_t i = 0; i < count && (offset + i) < RGB_MATRIX_LED_COUNT; i++) {
                led_buf[offset + i][0] = data[3 + i * 3];
                led_buf[offset + i][1] = data[4 + i * 3];
                led_buf[offset + i][2] = data[5 + i * 3];
            }
            break;
        }

        case CMD_BANDS:
            render_bands(&data[1], data[4], &data[5]);
            break;

        // A whole frame in one report: a background wash plus a handful of
        // per-LED overrides. Single-report so the renderer can never catch a
        // half-applied frame -- two reports would let the matrix draw between
        // them and flicker whatever the second one was going to fix up.
        case CMD_FRAME: {
            uint8_t count = data[4];
            if (count > FRAME_MAX_OVERRIDES) count = FRAME_MAX_OVERRIDES;
            for (uint8_t i = 0; i < RGB_MATRIX_LED_COUNT; i++) {
                led_buf[i][0] = data[1];
                led_buf[i][1] = data[2];
                led_buf[i][2] = data[3];
            }
            for (uint8_t i = 0; i < count; i++) {
                uint8_t led = data[5 + i * 4];
                if (led >= RGB_MATRIX_LED_COUNT) continue;
                led_buf[led][0] = data[6 + i * 4];
                led_buf[led][1] = data[7 + i * 4];
                led_buf[led][2] = data[8 + i * 4];
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
