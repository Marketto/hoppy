/*
 * Hoppy core for the iPhone/iPad app: C ABI over crates/hoppy-core (crates/hoppy-ffi).
 * Hand-written; a Rust test (crates/hoppy-ffi/src/tests.rs) keeps it in sync with the
 * exported functions. JSON formats and the event list: crates/hoppy-ffi/src/lib.rs.
 *
 * Threading: every function may be called from any thread, but hoppy_node_stop must not
 * race other calls on the same node. Events arrive on background threads; the callback
 * must return quickly and must not call hoppy_node_stop itself.
 */
#ifndef HOPPY_H
#define HOPPY_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct HoppyNode HoppyNode;

/* event_json is valid only during the call. */
typedef void (*hoppy_event_cb)(void *ctx, const char *event_json);

/* Starts a node from a JSON config: {"id","name","platform"?,"receive_dir"?,"udp_port"?,
 * "tcp_port"?,"receive"?,"multicast"?,"sweep"?,"seeds"?,"decision_timeout_secs"?}.
 * NULL on failure; *error_out (if not NULL) then gets a message to free. */
HoppyNode *hoppy_node_start(const char *config_json, hoppy_event_cb callback, void *ctx,
                            char **error_out);

/* Stops and frees the node; no callback runs after it returns. */
void hoppy_node_stop(HoppyNode *node);

/* JSON strings, free with hoppy_string_free. */
char *hoppy_node_info(const HoppyNode *node);
char *hoppy_node_peers(const HoppyNode *node);

void hoppy_node_refresh(const HoppyNode *node);

/* Port when on, 0 when off, -1 on error. */
int32_t hoppy_node_set_receiving(const HoppyNode *node, bool on);

/* paths_json: JSON array of absolute file paths. Returns a transfer id, 0 if invalid. */
uint64_t hoppy_node_send(const HoppyNode *node, const char *peer_id, const char *paths_json);

/* Answer an "offer" event. false if it is no longer pending. */
bool hoppy_node_decide(const HoppyNode *node, uint64_t offer, bool accept);

void hoppy_string_free(char *s);

/* Static string, do not free. */
const char *hoppy_version(void);

#ifdef __cplusplus
}
#endif

#endif /* HOPPY_H */
