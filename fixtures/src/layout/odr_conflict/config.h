// The layout of Config depends on a macro, so two translation units disagree.
// This is an ODR violation on purpose: libstratum must report both layouts.
#pragma once

struct Config {
    int id;
#ifdef CONFIG_WITH_NAME
    char name[16];
#endif
    double scale;
};

int config_scale_a();
int config_scale_b();
