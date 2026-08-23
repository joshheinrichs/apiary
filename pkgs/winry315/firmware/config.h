#pragma once

// The host paints every LED each frame via rgb_matrix_indicators_user(), so the
// underlying effect only needs to be the cheapest one available.
#define RGB_MATRIX_DEFAULT_MODE RGB_MATRIX_SOLID_COLOR
