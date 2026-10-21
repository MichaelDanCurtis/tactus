/* Smoke test: proves the generated header and static library are usable from
 * plain C, with no Rust toolchain knowledge required.
 *
 * Build (from the repo root):
 *   cargo build -p tactus-ffi
 *   cc -I crates/tactus-ffi/include -o /tmp/tactus_smoke examples/c/smoke.c \
 *      target/debug/libtactus.a \
 *      -framework CoreFoundation -framework IOKit -framework CoreMIDI \
 *      -framework CoreAudio -framework AppKit -lobjc -liconv
 *   /tmp/tactus_smoke
 *
 * The frameworks are needed even though this file calls nothing that uses
 * them: one static library carries the whole SDK, so linking it pulls in the
 * platform I/O layer regardless of which half you call. See examples/c/README
 * for the authoritative way to get this list for your platform.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include "tactus.h"

int main(void) {
    uint32_t v = tactus_abi_version();
    printf("ABI version %u.%u\n", v >> 16, v & 0xFFFF);
    printf("vendor id 0x%04X\n", tactus_vendor_id());

    /* Identify a model from a USB product id. */
    TactusModel model;
    int rc = tactus_model_from_product_id(0x2120, &model);
    assert(rc == TactusStatus_Ok);
    assert(model == TactusModel_S88);
    printf("0x2120 -> S88, %u keys\n", tactus_model_keys(model));

    /* A device that shares the vendor but is not ours must be rejected. */
    rc = tactus_model_from_product_id(0x1720, &model);
    assert(rc == TactusStatus_InvalidArgument);
    printf("0x1720 rejected: %s\n", tactus_status_name(rc));

    /* Light guide: one value per key, 0..127. */
    uint8_t keys[88];
    memset(keys, 0, sizeof keys);
    keys[40] = tactus_light_guide_max();
    uint8_t report[128];
    size_t n = 0;
    rc = tactus_encode_light_guide(TactusModel_S88, keys, 88, report, sizeof report, &n);
    assert(rc == TactusStatus_Ok);
    assert(n == 89 && report[0] == 0x82);
    printf("light guide report: %zu bytes, id 0x%02X\n", n, report[0]);

    /* Out of range must be refused, not masked. */
    keys[40] = 200;
    rc = tactus_encode_light_guide(TactusModel_S88, keys, 88, report, sizeof report, &n);
    assert(rc == TactusStatus_OutOfRange);
    printf("value 200 refused: %s\n", tactus_status_name(rc));
    keys[40] = 0;

    /* DAW SysEx with a text payload. */
    uint8_t midi[64];
    const char *name = "Bass";
    rc = tactus_daw_encode_text(0x48, 0, 2, name, strlen(name), midi, sizeof midi, &n);
    assert(rc == TactusStatus_Ok);
    assert(midi[0] == 0xF0 && midi[n - 1] == 0xF7);
    printf("track_name frame: %zu bytes, F0 ... F7\n", n);

    /* Non-ASCII must be distinguishable, not silently mangled. */
    const char *accented = "Caf\xC3\xA9";
    rc = tactus_daw_encode_text(0x48, 0, 0, accented, strlen(accented), midi, sizeof midi, &n);
    assert(rc == TactusStatus_NonAsciiText);
    printf("non-ascii refused: %s\n", tactus_status_name(rc));

    /* Relative encoder deltas. */
    assert(tactus_daw_decode_relative(0x01) == 1);
    assert(tactus_daw_decode_relative(0x7F) == -1);
    printf("relative encoder: 0x01 -> %d, 0x7F -> %d\n",
           tactus_daw_decode_relative(0x01), tactus_daw_decode_relative(0x7F));

    /* Handshake bytes. */
    uint8_t hs[9];
    rc = tactus_daw_handshake(hs, sizeof hs);
    assert(rc == TactusStatus_Ok);
    printf("handshake hello: %02X %02X %02X\n", hs[0], hs[1], hs[2]);

    /* Device-rendered page selection (MK3 renders its own UI). */
    printf("pages: browser=%u mixer=%u parameter=%u smartplay=%u\n",
           tactus_page_wire_byte(TactusPage_Browser),
           tactus_page_wire_byte(TactusPage_Mixer),
           tactus_page_wire_byte(TactusPage_Parameter),
           tactus_page_wire_byte(TactusPage_SmartPlay));

    /* Host-rendered pixel frame (MK2-class devices). */
    uint32_t px[8 * 2];
    for (int i = 0; i < 16; i++) px[i] = 0xFFFF0000u; /* opaque red */
    size_t cap = tactus_display_max_encoded_len(TactusPixelFormat_Rgb565, 8, 2);
    uint8_t *frame = malloc(cap);
    assert(frame != NULL);
    rc = tactus_display_encode_frame(0, TactusPixelFormat_Rgb565, 1,
                                     px, 8, 2, 8, 0, 0, 8, 2, NULL,
                                     frame, cap, &n);
    assert(rc == TactusStatus_Ok);
    assert(frame[0] == 0x84 && frame[1] == 0x00);
    printf("pixel frame: %zu bytes, header %02X %02X, fmt %02X\n",
           n, frame[0], frame[1], frame[3]);
    free(frame);

    /* A null pointer must produce an error, never a crash. */
    rc = tactus_model_from_product_id(0x2120, NULL);
    assert(rc == TactusStatus_NullArgument);
    printf("null argument refused: %s\n", tactus_status_name(rc));

    printf("\nall C ABI assertions passed\n");
    return 0;
}
