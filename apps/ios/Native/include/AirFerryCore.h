#ifndef AIRFERRY_CORE_H
#define AIRFERRY_CORE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void *airferry_sender_create(const uint8_t *payload, size_t payload_len,
                             const uint8_t *filename_utf8, size_t filename_len,
                             uint64_t modified_ms, uint8_t redundancy_pct,
                             uint32_t symbol_size);
void airferry_sender_destroy(void *handle);
size_t airferry_sender_frame_capacity(const void *handle);
size_t airferry_sender_next_frame(void *handle, uint8_t *out, size_t cap);
uint32_t airferry_sender_segment_count(const void *handle);
uint32_t airferry_sender_segment_index(const void *handle);
int32_t airferry_sender_select_segment(void *handle, uint32_t segment_index);
uint32_t airferry_sender_total_symbols(const void *handle);

void *airferry_receiver_create(uint64_t sid_lo, uint64_t sid_hi);
void *airferry_receiver_create_from_frame(const uint8_t *frame_bytes, size_t frame_len);
void airferry_receiver_destroy(void *handle);
uint64_t airferry_receiver_ingest(void *handle, const uint8_t *frame_bytes, size_t frame_len);
int32_t airferry_receiver_is_complete(const void *handle);
int32_t airferry_receiver_assemble(void *handle, uint8_t **out_buf, size_t *out_len);
int32_t airferry_receiver_assemble_raw(void *handle, uint8_t **out_buf, size_t *out_len);
void airferry_buffer_free(uint8_t *ptr, size_t len);
uint8_t airferry_receiver_compression(const void *handle);
uint64_t airferry_receiver_compressed_size(const void *handle);
uint64_t airferry_receiver_original_size(const void *handle);
size_t airferry_receiver_progress_json(const void *handle, uint8_t *out, size_t cap);
size_t airferry_receiver_file_name(const void *handle, uint8_t *out, size_t cap);
uint64_t airferry_receiver_file_size(const void *handle);
uint64_t airferry_receiver_crc32(const void *handle);
int32_t airferry_receiver_crc32_known(const void *handle);
int32_t airferry_receiver_is_segmented(const void *handle);
uint32_t airferry_receiver_segment_index(const void *handle);
uint32_t airferry_receiver_segment_count(const void *handle);
uint64_t airferry_receiver_root_original_size(const void *handle);
uint64_t airferry_receiver_original_offset(const void *handle);
uint64_t airferry_receiver_root_session_id_lo(const void *handle);
uint64_t airferry_receiver_root_session_id_hi(const void *handle);
size_t airferry_receiver_raw_sha256(const void *handle, uint8_t *out, size_t cap);
size_t airferry_receiver_root_sha256(const void *handle, uint8_t *out, size_t cap);
int32_t airferry_decompress_stream_to_file(const char *input_path,
                                           const char *output_path,
                                           uint8_t compression,
                                           uint64_t max_output,
                                           uint64_t expected_size,
                                           uint32_t expected_crc,
                                           bool crc_known,
                                           const char *expected_sha_hex);

#ifdef __cplusplus
}
#endif

#endif

