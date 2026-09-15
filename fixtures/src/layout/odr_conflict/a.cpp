#define CONFIG_WITH_NAME
#include "config.h"

Config g_config_a;  // external linkage: a static would be optimized out with its type info at -O2

int config_scale_a() {
    return static_cast<int>(g_config_a.scale) + g_config_a.name[0];
}

int main() {
    return config_scale_a() + config_scale_b();
}
