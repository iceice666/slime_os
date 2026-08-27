#include <slime/component_runtime.h>

void slime_component_main(uint32_t startup_arg)
{
    static const uint8_t marker[] = "[c-runtime-probe] C component ready\n";
    (void)startup_arg;
    slime_debug_write(marker, sizeof(marker) - 1U);
}
