#include <pipewire/pipewire.h>

/* libclang 22 no longer exports these casted macros as bindgen constants. */
#undef SPA_ID_INVALID
#define SPA_ID_INVALID 0xffffffffU
#undef PW_ID_ANY
#define PW_ID_ANY 0xffffffffU
