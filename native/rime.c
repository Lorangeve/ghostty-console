#include <rime_api.h>
#include <stdlib.h>

typedef struct {
    RimeApi *api;
    RimeSessionId session;
    RimeContext context;
    RimeCommit commit;
    int has_context, has_commit;
} ConsoleRime;

void cr_close(ConsoleRime *r) {
    if (!r) return;
    if (r->has_context) r->api->free_context(&r->context);
    if (r->has_commit) r->api->free_commit(&r->commit);
    if (r->session) r->api->destroy_session(r->session);
    r->api->finalize();
    free(r);
}
ConsoleRime *cr_open(const char *shared, const char *user, const char *prebuilt,
                     const char *schema) {
    ConsoleRime *r = calloc(1, sizeof(*r));
    if (!r) return NULL;
    r->api = rime_get_api();
    RIME_STRUCT(RimeTraits, traits);
    traits.shared_data_dir = shared;
    traits.user_data_dir = user;
    traits.prebuilt_data_dir = prebuilt;
    traits.app_name = "rime.ghostty-console";
    traits.distribution_name = "Ghostty Console";
    traits.distribution_code_name = "ghostty-console";
    traits.distribution_version = "0.1.0";
    traits.min_log_level = 2;
    traits.log_dir = "";
    r->api->setup(&traits);
    r->api->initialize(&traits);
    r->session = r->api->create_session();
    if (!r->session || !r->api->select_schema(r->session, schema)) {
        cr_close(r);
        return NULL;
    }
    r->api->set_option(r->session, "ascii_mode", False);
    return r;
}
int cr_key(ConsoleRime *r, int key, int modifiers) {
    if (r->has_context) r->api->free_context(&r->context);
    if (r->has_commit) r->api->free_commit(&r->commit);
    r->has_context = r->has_commit = 0;
    int handled = r->api->process_key(r->session, key, modifiers);
    RIME_STRUCT_INIT(RimeContext, r->context);
    RIME_STRUCT_INIT(RimeCommit, r->commit);
    r->has_context = r->api->get_context(r->session, &r->context);
    r->has_commit = r->api->get_commit(r->session, &r->commit);
    return handled;
}
const char *cr_commit(ConsoleRime *r) { return r->has_commit ? r->commit.text : NULL; }
const char *cr_preedit(ConsoleRime *r) { return r->has_context ? r->context.composition.preedit : NULL; }
int cr_meta(ConsoleRime *r, int field) {
    if (!r->has_context) return 0;
    switch (field) {
    case 0: return r->context.composition.cursor_pos;
    case 1: return r->context.menu.page_no;
    case 2: return r->context.menu.highlighted_candidate_index;
    case 3: return r->context.menu.num_candidates;
    case 4: return r->context.menu.is_last_page;
    case 5: return r->context.composition.sel_start;
    case 6: return r->context.composition.sel_end;
    default: return 0;
    }
}
const char *cr_candidate(ConsoleRime *r, int index, int field) {
    if (!r->has_context || index < 0 || index >= r->context.menu.num_candidates) return NULL;
    if (field == 0) return r->context.menu.candidates[index].text;
    if (field == 1) return r->context.menu.candidates[index].comment;
    if (r->context.select_labels) return r->context.select_labels[index];
    return NULL;
}
void cr_clear(ConsoleRime *r) { r->api->clear_composition(r->session); }
