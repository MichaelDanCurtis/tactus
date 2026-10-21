/* Drives real hardware through the C ABI: enumerate, open, identify, read,
 * poll, light the keybed, resolve MIDI ports.
 *
 * Separate from smoke.c because that one is pure protocol and runs anywhere,
 * while this one needs a keyboard plugged in. With nothing attached it prints
 * what it found and exits 0, so it is still safe to run unattended.
 *
 * Build (from the repo root):
 *   cargo build -p tactus-ffi
 *   cc -Wall -Wextra -I crates/tactus-ffi/include -o /tmp/tactus_device \
 *      examples/c/device.c target/debug/libtactus.a \
 *      -framework CoreFoundation -framework IOKit -framework CoreMIDI \
 *      -framework CoreAudio -framework AppKit
 *   /tmp/tactus_device
 *
 * This program does not send MIDI. Opening a port is harmless; transmitting a
 * command set recovered from a vendor script into a keyboard somebody else's
 * software is currently driving is not, so it resolves the ports and stops.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include "tactus.h"

/* Print the diagnostic the SDK recorded for the last failure. */
static void print_last_error(const char *context) {
    size_t len = 0;
    if (tactus_last_error_message(NULL, 0, &len) != TactusStatus_Ok) {
        printf("  %s: (no diagnostic available)\n", context);
        return;
    }
    if (len == 0) {
        printf("  %s: (no diagnostic recorded)\n", context);
        return;
    }
    char *buf = malloc(len + 1);
    if (buf == NULL) {
        printf("  %s: (out of memory)\n", context);
        return;
    }
    if (tactus_last_error_message(buf, len + 1, NULL) == TactusStatus_Ok) {
        printf("  %s: %s\n", context, buf);
    }
    free(buf);
}

int main(void) {
    uint32_t v = tactus_abi_version();
    printf("tactus ABI %u.%u\n\n", v >> 16, v & 0xFFFF);

    /* ---- Enumerate ---------------------------------------------------- */
    TactusDeviceList *list = NULL;
    int rc = tactus_device_list_new(&list);
    if (rc != TactusStatus_Ok) {
        printf("enumeration failed: %s\n", tactus_status_name(rc));
        print_last_error("reason");
        return 1;
    }
    size_t count = tactus_device_list_count(list);
    printf("%zu keyboard(s) attached\n", count);

    for (size_t i = 0; i < count; i++) {
        TactusDeviceInfo info;
        rc = tactus_device_list_get(list, i, &info);
        assert(rc == TactusStatus_Ok);
        printf("  [%zu] pid 0x%04X  usb_serial %s  product %s  iface %d\n",
               i,
               info.product_id,
               info.usb_serial ? info.usb_serial : "(none)",
               info.product_string ? info.product_string : "(none)",
               info.interface_number);
    }

    /* Reading past the end is an error, not a crash. */
    TactusDeviceInfo oob;
    rc = tactus_device_list_get(list, count + 10, &oob);
    assert(rc == TactusStatus_OutOfRange);
    printf("index past the end: %s\n", tactus_status_name(rc));

    if (count == 0) {
        printf("\nnothing attached; the protocol paths are covered by smoke.c\n");
        tactus_device_list_free(list);
        return 0;
    }

    /* ---- Open --------------------------------------------------------- */
    TactusDevice *device = NULL;
    rc = tactus_device_open_at(list, 0, &device);
    if (rc != TactusStatus_Ok) {
        printf("open failed: %s\n", tactus_status_name(rc));
        print_last_error("reason");
        tactus_device_list_free(list);
        return 1;
    }
    /* The list owns the strings we printed, so it must outlive their use --
     * which it did. Freeing it here proves the device handle is independent. */
    tactus_device_list_free(list);
    list = NULL;
    printf("\nopened\n");

    TactusModel model;
    rc = tactus_device_model(device, &model);
    assert(rc == TactusStatus_Ok);
    printf("  model: %u keys\n", tactus_model_keys(model));

    /* The policy that matters: we must NOT have taken the device away from
     * whatever else is using it. macOS HID opens are exclusive by default. */
    bool exclusive = true;
    rc = tactus_device_is_open_exclusive(device, &exclusive);
    assert(rc == TactusStatus_Ok);
    printf("  opened exclusively (seizing the device)? %s\n",
           exclusive ? "YES - BUG" : "no");
    assert(!exclusive);

    /* ---- Identity ----------------------------------------------------- */
    size_t len = 0;
    rc = tactus_device_serial(device, NULL, 0, &len);
    assert(rc == TactusStatus_Ok);
    char serial[128];
    if (len + 1 <= sizeof serial) {
        rc = tactus_device_serial(device, serial, sizeof serial, NULL);
        assert(rc == TactusStatus_Ok);
        printf("  0xD9 serial: \"%s\" (%zu chars)\n", serial, len);
    }

    /* A buffer one byte too small must report the need, not truncate. */
    char tiny[2];
    rc = tactus_device_serial(device, tiny, sizeof tiny, &len);
    assert(rc == TactusStatus_BufferTooSmall);
    printf("  short buffer: %s, needs %zu+1\n", tactus_status_name(rc), len);

    /* ---- Feature reports ---------------------------------------------- */
    uint8_t feature[TACTUS_FEATURE_BUFFER_LEN];
    size_t flen = 0;
    rc = tactus_device_read_feature(device, 0xD8, feature, sizeof feature, &flen);
    assert(rc == TactusStatus_Ok);
    printf("  0xD8 (%zu bytes):", flen);
    for (size_t i = 0; i < flen && i < 10; i++) printf(" %02X", feature[i]);
    printf(" ...\n");

    /* 0xF8 appears in the vendor's published report-map table but this
     * firmware does not declare it. Refused before it reaches the device. */
    rc = tactus_device_read_feature(device, 0xF8, feature, sizeof feature, &flen);
    assert(rc == TactusStatus_Refused);
    printf("  read 0xF8: %s\n", tactus_status_name(rc));
    print_last_error("why");

    /* Writing a feature report is refused by policy, with a reason. */
    uint8_t zeros[TACTUS_FEATURE_PAYLOAD_LEN];
    memset(zeros, 0, sizeof zeros);
    rc = tactus_device_write_feature(device, 0xD0, zeros, sizeof zeros);
    assert(rc == TactusStatus_Refused);
    printf("  write 0xD0: %s\n", tactus_status_name(rc));
    print_last_error("why");

    /* ---- Input -------------------------------------------------------- */
    printf("\npolling 2s for input (touch something, or don't)\n");
    int reports = 0;
    for (int i = 0; i < 20; i++) {
        TactusInputReport report;
        bool got = false;
        rc = tactus_device_poll_input(device, 100, &report, &got);
        if (rc != TactusStatus_Ok) {
            printf("  poll failed: %s\n", tactus_status_name(rc));
            print_last_error("reason");
            break;
        }
        if (!got) continue;
        reports++;
        printf("  report 0x%02X kind %u", report.report_id, report.kind);
        if (report.kind == TactusInputKind_Analog) {
            printf(" continuous[0..4] %u %u %u %u",
                   report.continuous[0], report.continuous[1],
                   report.continuous[2], report.continuous[3]);
        } else if (report.kind == TactusInputKind_Wheels) {
            printf(" full_range[0] %u", report.full_range[0]);
        }
        printf("\n");
        if (reports >= 8) break;
    }
    printf("  %d report(s); zero is expected when idle\n", reports);

    /* ---- Light guide -------------------------------------------------- */
    /* An out-of-range value is refused rather than masked. */
    uint8_t lights[TACTUS_MAX_KEYS];
    memset(lights, 0, sizeof lights);
    lights[0] = TACTUS_LIGHT_GUIDE_MAX + 1;
    rc = tactus_device_set_light_guide(device, lights, tactus_model_keys(model));
    assert(rc == TactusStatus_OutOfRange);
    printf("\n  value %u refused: %s\n", lights[0], tactus_status_name(rc));
    print_last_error("why");

    /* A buffer sized for the wrong model is refused, not truncated. */
    rc = tactus_device_set_light_guide(device, lights, 49);
    assert(rc != TactusStatus_Ok);
    printf("  49 values on this keybed: %s\n", tactus_status_name(rc));

    /* All off: a device-declared report, a device-declared range. */
    rc = tactus_device_clear_light_guide(device);
    if (rc == TactusStatus_Ok) {
        printf("  light guide cleared\n");
    } else {
        printf("  clear failed: %s\n", tactus_status_name(rc));
        print_last_error("reason");
    }

    tactus_device_free(device);

    /* ---- MIDI port resolution ----------------------------------------- */
    printf("\nresolving MIDI ports (opening only; nothing is transmitted)\n");
    const char *role_name[] = {"Main", "DAW", "Ext"};
    TactusPortRole roles[] = {TactusPortRole_Main, TactusPortRole_Daw, TactusPortRole_Ext};
    for (int i = 0; i < 3; i++) {
        TactusMidiOut *out = NULL;
        rc = tactus_midi_out_open(roles[i], "KONTROL", &out);
        if (rc == TactusStatus_Ok) {
            printf("  %s out: open\n", role_name[i]);
            tactus_midi_out_free(out);
        } else {
            printf("  %s out: %s\n", role_name[i], tactus_status_name(rc));
            print_last_error("reason");
        }
    }

    /* A role on a device that is not there must not resolve to something else. */
    TactusMidiOut *nope = NULL;
    rc = tactus_midi_out_open(TactusPortRole_Daw, "NO SUCH KEYBOARD", &nope);
    assert(rc != TactusStatus_Ok);
    printf("  bogus filter: %s\n", tactus_status_name(rc));
    print_last_error("why");

    /* An unrecognised command id is refused before anything is encoded. */
    TactusMidiOut *main_out = NULL;
    if (tactus_midi_out_open(TactusPortRole_Main, "KONTROL", &main_out) == TactusStatus_Ok) {
        rc = tactus_midi_out_send_command(main_out, 0x7E, 0, 0, NULL, 0);
        assert(rc == TactusStatus_InvalidArgument);
        printf("  command id 0x7E: %s\n", tactus_status_name(rc));
        tactus_midi_out_free(main_out);
    }

    /* Freeing null is a no-op, so cleanup paths need no guards. */
    tactus_device_free(NULL);
    tactus_device_list_free(NULL);
    tactus_midi_out_free(NULL);
    tactus_midi_in_free(NULL);

    printf("\nall assertions passed\n");
    return 0;
}
