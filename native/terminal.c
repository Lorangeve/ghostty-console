#include <ghostty/vt.h>
#include <stdlib.h>
#include <string.h>
#include <linux/input-event-codes.h>

typedef void (*Reply)(void *, const unsigned char *, size_t);
typedef struct {
    GhosttyTerminal terminal;
    GhosttyRenderState render;
    GhosttyRenderStateRowIterator rows;
    GhosttyRenderStateRowCells cells;
    GhosttyKeyEncoder encoder;
    GhosttyKeyEvent event;
    GhosttyRenderStateImageIterator images;
    GhosttySizeReportSize size;
    GhosttyMouseEncoder mouse_encoder;
    GhosttyMouseEvent mouse_event;
    uint8_t *clipboard; size_t clipboard_len; bool clipboard_pending;
    Reply reply;
    void *userdata;
} Terminal;
typedef struct { unsigned char text[64]; unsigned int fg, bg; unsigned char width, flags; } Cell;
static unsigned int rgb(GhosttyColorRgb c) { return (c.r<<16)|(c.g<<8)|c.b; }
static void write_reply(GhosttyTerminal t, void *data, const uint8_t *bytes, size_t len) {
    (void)t; Terminal *v = data; if (v->reply) v->reply(v->userdata, bytes, len);
}
void gt_free(Terminal *v) {
    if (!v) return;
    free(v->clipboard);
    ghostty_mouse_event_free(v->mouse_event);
    ghostty_mouse_encoder_free(v->mouse_encoder);
    ghostty_render_state_image_iterator_free(v->images);
    ghostty_key_event_free(v->event); ghostty_key_encoder_free(v->encoder);
    ghostty_render_state_row_cells_free(v->cells);
    ghostty_render_state_row_iterator_free(v->rows);
    ghostty_render_state_free(v->render); ghostty_terminal_free(v->terminal); free(v);
}
static bool size_report(GhosttyTerminal terminal, void *data, GhosttySizeReportSize *out) {
    (void)terminal; *out = ((Terminal *)data)->size; return true;
}
static bool attributes(GhosttyTerminal terminal, void *data, GhosttyDeviceAttributes *out) {
    (void)terminal; (void)data; memset(out, 0, sizeof(*out));
    out->primary.conformance_level = GHOSTTY_DA_CONFORMANCE_VT220;
    out->primary.features[0] = GHOSTTY_DA_FEATURE_ANSI_COLOR;
    out->primary.num_features = 1;
    out->secondary.device_type = GHOSTTY_DA_DEVICE_TYPE_VT220;
    return true;
}
static GhosttyString version(GhosttyTerminal terminal, void *data) {
    (void)terminal; (void)data;
    const char *text = "ghostty-console 0.1.0";
    return (GhosttyString){ .ptr = (const uint8_t *)text, .len = strlen(text) };
}
static void clipboard_write(GhosttyTerminal terminal, void *data, const GhosttyClipboardWrite *request) {
    (void)terminal; Terminal *v=data;
    GhosttyClipboardWriteReply reply=GHOSTTY_INIT_SIZED(GhosttyClipboardWriteReply);
    reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    for (size_t i=0;i<request->contents_len;i++) {
        const GhosttyClipboardContent *content=&request->contents[i];
        if (content->mime.len<10 || memcmp(content->mime.ptr,"text/plain",10)) continue;
        if (content->data.len>1024*1024) {reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;break;}
        uint8_t *text=malloc(content->data.len+1);
        if (!text) {reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_IO_ERROR;break;}
        memcpy(text,content->data.ptr,content->data.len);text[content->data.len]=0;
        free(v->clipboard);v->clipboard=text;v->clipboard_len=content->data.len;v->clipboard_pending=true;
        reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;break;
    }
    request->reply(request,&reply);
}
Terminal *gt_new(unsigned short cols, unsigned short rows, Reply reply, void *data) {
    Terminal *v = calloc(1, sizeof(*v)); if (!v) return NULL;
    if (ghostty_terminal_new(NULL, &v->terminal, cols, rows) ||
        ghostty_render_state_new(NULL, &v->render) ||
        ghostty_render_state_row_iterator_new(NULL, &v->rows) ||
        ghostty_render_state_row_cells_new(NULL, &v->cells) ||
        ghostty_key_encoder_new(NULL, &v->encoder) || ghostty_key_event_new(NULL, &v->event) ||
        ghostty_render_state_image_iterator_new(NULL, &v->images) ||
        ghostty_mouse_encoder_new(NULL, &v->mouse_encoder) || ghostty_mouse_event_new(NULL, &v->mouse_event)) {
        gt_free(v); return NULL;
    }
    v->reply = reply; v->userdata = data;
    v->size = (GhosttySizeReportSize){ .rows = rows, .columns = cols, .cell_width = 8, .cell_height = 16 };
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_USERDATA, v);
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_reply);
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE, clipboard_write);
    size_t clipboard_limit=1024*1024;
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES, &clipboard_limit);
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SIZE, size_report);
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES, attributes);
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_XTVERSION, version);
    uint64_t image_limit = 64*1024*1024;
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT, &image_limit);
    bool enable = true;
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE, &enable);
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM, &enable);
    size_t limit = 8*1024*1024;
    ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &limit);
    return v;
}
void gt_write(Terminal *v, const unsigned char *s, size_t n) { ghostty_terminal_vt_write(v->terminal, s, n); }
int gt_frame(Terminal *v, Cell *out, size_t capacity, int *cursor_x, int *cursor_y, int *caret_x, int *caret_y) {
    if (ghostty_render_state_update(v->render, v->terminal)) return -1;
    GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
    ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors);
    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor);
    *caret_x = cursor.viewport_has_value ? cursor.viewport_x : -1;
    *caret_y = cursor.viewport_has_value ? cursor.viewport_y : -1;
    *cursor_x = cursor.visible && cursor.viewport_has_value ? cursor.viewport_x : -1;
    *cursor_y = cursor.visible && cursor.viewport_has_value ? cursor.viewport_y : -1;
    ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &v->rows);
    size_t n = 0;
    while (ghostty_render_state_row_iterator_next(v->rows)) {
        GhosttyRow row=0; bool wrapped=false;
        ghostty_render_state_row_get(v->rows, GHOSTTY_RENDER_STATE_ROW_DATA_RAW, &row);
        ghostty_row_get(row,GHOSTTY_ROW_DATA_WRAP,&wrapped);
        ghostty_render_state_row_get(v->rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &v->cells);
        while (ghostty_render_state_row_cells_next(v->cells)) {
            if (n >= capacity) return -1;
            Cell *c = &out[n++]; memset(c, 0, sizeof(*c));
            GhosttyColorRgb fg = colors.foreground, bg = colors.background;
            ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg);
            ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg);
            GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
            ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);
            if (style.inverse) { GhosttyColorRgb temp = fg; fg = bg; bg = temp; }
            c->fg = rgb(fg); c->bg = rgb(bg); c->flags = (style.bold?1:0)|(style.underline?2:0)|(wrapped?4:0);
            if (style.invisible) c->fg = c->bg;
            GhosttyCell raw = 0;
            GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
            ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw);
            ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &wide);
            c->width = wide == GHOSTTY_CELL_WIDE_WIDE ? 2 : (wide == GHOSTTY_CELL_WIDE_NARROW ? 1 : 0);
            GhosttyBuffer buf = { .ptr = c->text, .cap = sizeof(c->text)-1, .len = 0 };
            if (ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &buf)) {
                memcpy(c->text, "\xef\xbf\xbd", 3);
            }
        }
    }
    return (int)n;
}
static GhosttyKey key_from_evdev(unsigned int k) {
#define K(l,g) case KEY_##l: return GHOSTTY_KEY_##g;
    switch (k) {
    K(ESC,ESCAPE) K(ENTER,ENTER) K(BACKSPACE,BACKSPACE) K(TAB,TAB) K(SPACE,SPACE)
    K(UP,ARROW_UP) K(DOWN,ARROW_DOWN) K(LEFT,ARROW_LEFT) K(RIGHT,ARROW_RIGHT)
    K(HOME,HOME) K(END,END) K(PAGEUP,PAGE_UP) K(PAGEDOWN,PAGE_DOWN) K(INSERT,INSERT) K(DELETE,DELETE)
    K(LEFTCTRL,CONTROL_LEFT) K(RIGHTCTRL,CONTROL_RIGHT) K(LEFTSHIFT,SHIFT_LEFT) K(RIGHTSHIFT,SHIFT_RIGHT)
    K(LEFTALT,ALT_LEFT) K(RIGHTALT,ALT_RIGHT) K(LEFTMETA,META_LEFT) K(RIGHTMETA,META_RIGHT) K(CAPSLOCK,CAPS_LOCK)
    K(MINUS,MINUS) K(EQUAL,EQUAL) K(LEFTBRACE,BRACKET_LEFT) K(RIGHTBRACE,BRACKET_RIGHT)
    K(SEMICOLON,SEMICOLON) K(APOSTROPHE,QUOTE) K(GRAVE,BACKQUOTE) K(BACKSLASH,BACKSLASH)
    K(COMMA,COMMA) K(DOT,PERIOD) K(SLASH,SLASH)
    K(A,A) K(B,B) K(C,C) K(D,D) K(E,E) K(F,F) K(G,G) K(H,H) K(I,I) K(J,J) K(K,K) K(L,L) K(M,M)
    K(N,N) K(O,O) K(P,P) K(Q,Q) K(R,R) K(S,S) K(T,T) K(U,U) K(V,V) K(W,W) K(X,X) K(Y,Y) K(Z,Z)
    K(0,DIGIT_0) K(1,DIGIT_1) K(2,DIGIT_2) K(3,DIGIT_3) K(4,DIGIT_4) K(5,DIGIT_5)
    K(6,DIGIT_6) K(7,DIGIT_7) K(8,DIGIT_8) K(9,DIGIT_9)
    K(F1,F1) K(F2,F2) K(F3,F3) K(F4,F4) K(F5,F5) K(F6,F6) K(F7,F7) K(F8,F8) K(F9,F9) K(F10,F10) K(F11,F11) K(F12,F12)
    default: return GHOSTTY_KEY_UNIDENTIFIED;
    }
#undef K
}
int gt_key(Terminal *v, unsigned int evdev, int action, unsigned int mods,
           const char *text, size_t len, unsigned int unshifted, char *out, size_t cap) {
    ghostty_key_encoder_setopt_from_terminal(v->encoder, v->terminal);
    ghostty_key_event_set_key(v->event, key_from_evdev(evdev));
    ghostty_key_event_set_action(v->event, action);
    ghostty_key_event_set_mods(v->event, mods);
    ghostty_key_event_set_consumed_mods(v->event, 0);
    ghostty_key_event_set_utf8(v->event, text, len);
    ghostty_key_event_set_unshifted_codepoint(v->event, unshifted);
    size_t n = 0;
    if (ghostty_key_encoder_encode(v->encoder, v->event, out, cap, &n)) return -1;
    return (int)n;
}

int gt_cell_size(Terminal *v, uint32_t width, uint32_t height) {
    v->size.cell_width = width; v->size.cell_height = height;
    return ghostty_terminal_resize(v->terminal, v->size.columns, v->size.rows, width, height);
}
typedef struct {
    int32_t col, row, z, layer;
    uint32_t offset_x, offset_y, dest_width, dest_height;
    uint32_t source_x, source_y, source_width, source_height;
    uint32_t width, height, format, id;
    uint64_t generation, storage_id;
    const uint8_t *pixels;
    size_t len;
} Image;
int gt_images_begin(Terminal *v) {
    return ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR, &v->images);
}
int gt_image_next(Terminal *v, Image *out) {
    GhosttyKittyGraphics storage = NULL;
    if (ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, &storage)) return 0;
    while (ghostty_render_state_image_iterator_next(v->images)) {
        GhosttyImageProtocol protocol;
        if (ghostty_render_state_image_get(v->images, GHOSTTY_RENDER_STATE_IMAGE_DATA_PROTOCOL, &protocol) || protocol != GHOSTTY_IMAGE_PROTOCOL_KITTY) continue;
        memset(out, 0, sizeof(*out));out->storage_id=(uint64_t)(uintptr_t)storage;
#define FIELD(name, field) if (ghostty_render_state_image_get(v->images, GHOSTTY_RENDER_STATE_IMAGE_DATA_##name, &out->field)) continue
        FIELD(IMAGE_ID, id); FIELD(IMAGE_GENERATION, generation);
        FIELD(LAYER, layer); FIELD(Z, z); FIELD(VIEWPORT_COL, col); FIELD(VIEWPORT_ROW, row);
        FIELD(CELL_OFFSET_X, offset_x); FIELD(CELL_OFFSET_Y, offset_y);
        FIELD(PIXEL_WIDTH, dest_width); FIELD(PIXEL_HEIGHT, dest_height);
        FIELD(SOURCE_X, source_x); FIELD(SOURCE_Y, source_y);
        FIELD(SOURCE_WIDTH, source_width); FIELD(SOURCE_HEIGHT, source_height);
#undef FIELD
        GhosttyKittyGraphicsImage image = ghostty_kitty_graphics_image(storage, out->id);
        if (!image) continue;
#define FIELD(name, field) if (ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_IMAGE_DATA_##name, &out->field)) continue
        FIELD(WIDTH, width); FIELD(HEIGHT, height); FIELD(FORMAT, format);
        FIELD(DATA_PTR, pixels); FIELD(DATA_LEN, len);
#undef FIELD
        return 1;
    }
    return 0;
}
int gt_resize(Terminal *v, uint16_t cols, uint16_t rows) {
    v->size.columns = cols; v->size.rows = rows;
    return ghostty_terminal_resize(v->terminal, cols, rows, v->size.cell_width, v->size.cell_height);
}
int gt_mouse_tracking(Terminal *v) {
    bool active = false;
    ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, &active);
    return active;
}
int gt_bracketed_paste(Terminal *v) {
    GhosttyTerminalModeConfig mode = { .mode = GHOSTTY_MODE_BRACKETED_PASTE };
    ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_MODE, &mode);
    return mode.value;
}
int gt_mouse(Terminal *v, int action, int button, float x, float y, unsigned int mods, char *out, size_t cap) {
    ghostty_mouse_encoder_setopt_from_terminal(v->mouse_encoder, v->terminal);
    GhosttyMouseEncoderSize size = GHOSTTY_INIT_SIZED(GhosttyMouseEncoderSize);
    size.cell_width = v->size.cell_width; size.cell_height = v->size.cell_height;
    size.screen_width = v->size.columns * size.cell_width;
    size.screen_height = v->size.rows * size.cell_height;
    ghostty_mouse_encoder_setopt(v->mouse_encoder, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size);
    bool held = button != 0 && action != GHOSTTY_MOUSE_ACTION_RELEASE;
    ghostty_mouse_encoder_setopt(v->mouse_encoder, GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, &held);
    ghostty_mouse_event_set_action(v->mouse_event, action);
    if (button) ghostty_mouse_event_set_button(v->mouse_event, button);
    else ghostty_mouse_event_clear_button(v->mouse_event);
    ghostty_mouse_event_set_position(v->mouse_event, (GhosttyMousePosition){ .x = x, .y = y });
    ghostty_mouse_event_set_mods(v->mouse_event, mods);
    size_t len = 0;
    if (ghostty_mouse_encoder_encode(v->mouse_encoder, v->mouse_event, out, cap, &len)) return -1;
    return (int)len;
}

int gt_clipboard(Terminal *v, const uint8_t **out, size_t *len) {
    if (!v->clipboard_pending) return 0;
    *out=v->clipboard;*len=v->clipboard_len;v->clipboard_pending=false;return 1;
}
void gt_colors(Terminal *v, uint32_t foreground, uint32_t background, uint32_t cursor) {
    uint32_t values[]={foreground,background,cursor};
    GhosttyTerminalOption options[]={GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,GHOSTTY_TERMINAL_OPT_COLOR_CURSOR};
    for (int i=0;i<3;i++) {if (values[i]>0xffffff) continue;
        GhosttyColorRgb color={.r=values[i]>>16,.g=values[i]>>8,.b=values[i]};
        ghostty_terminal_set(v->terminal,options[i],&color);
    }
}
