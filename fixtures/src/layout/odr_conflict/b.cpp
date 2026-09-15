#include "config.h"

Config g_config_b;

int config_scale_b() {
    return static_cast<int>(g_config_b.scale) + g_config_b.id;
}
