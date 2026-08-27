#include "slisp.h"

#include <stdint.h>

#define SLISP_MAX_NODES 192
#define SLISP_MAX_SYMBOL_BYTES 16
#define SLISP_MAX_BINDINGS 96
#define SLISP_MAX_EVAL_DEPTH 48

typedef uint16_t NodeRef;
typedef uint16_t EnvRef;

enum NodeKind {
    NODE_NIL,
    NODE_BOOL,
    NODE_INT,
    NODE_SYMBOL,
    NODE_PAIR,
    NODE_CLOSURE
};

typedef struct {
    uint8_t kind;
    union {
        int64_t integer;
        uint8_t boolean;
        char symbol[SLISP_MAX_SYMBOL_BYTES];
        struct {
            NodeRef car;
            NodeRef cdr;
        } pair;
        struct {
            NodeRef parameters;
            NodeRef body;
            EnvRef environment;
        } closure;
    } value;
} Node;

typedef struct {
    char name[SLISP_MAX_SYMBOL_BYTES];
    NodeRef value;
    EnvRef parent;
} Binding;

typedef struct {
    const char *source;
    size_t cursor;
    Node nodes[SLISP_MAX_NODES];
    uint16_t node_count;
    Binding bindings[SLISP_MAX_BINDINGS];
    uint16_t binding_count;
    EnvRef global_environment;
} State;

static size_t text_len(const char *text)
{
    size_t len = 0;
    while (text[len] != '\0') {
        ++len;
    }
    return len;
}

static int text_equal(const char *left, const char *right)
{
    size_t index = 0;
    while (left[index] == right[index]) {
        if (left[index] == '\0') {
            return 1;
        }
        ++index;
    }
    return 0;
}

static void text_copy(char *destination, const char *source, size_t capacity)
{
    size_t index = 0;
    if (capacity == 0) {
        return;
    }
    while (source[index] != '\0' && index + 1 < capacity) {
        destination[index] = source[index];
        ++index;
    }
    destination[index] = '\0';
}

static int is_space(char character)
{
    return character == ' ' || character == '\n' || character == '\r' || character == '\t';
}

static int is_digit(char character)
{
    return character >= '0' && character <= '9';
}

static int is_delimiter(char character)
{
    return character == '\0' || is_space(character) || character == '(' || character == ')';
}

static void skip_space(State *state)
{
    while (is_space(state->source[state->cursor])) {
        ++state->cursor;
    }
}

static SlispStatus allocate_node(State *state, uint8_t kind, NodeRef *result)
{
    NodeRef node;
    if (state->node_count >= SLISP_MAX_NODES) {
        return SLISP_ERR_LIMIT;
    }
    node = state->node_count++;
    state->nodes[node].kind = kind;
    *result = node;
    return SLISP_OK;
}

static SlispStatus make_pair(State *state, NodeRef car, NodeRef cdr, NodeRef *result)
{
    SlispStatus status = allocate_node(state, NODE_PAIR, result);
    if (status != SLISP_OK) {
        return status;
    }
    state->nodes[*result].value.pair.car = car;
    state->nodes[*result].value.pair.cdr = cdr;
    return SLISP_OK;
}

static SlispStatus parse_expression(State *state, NodeRef *result);

static SlispStatus parse_list(State *state, NodeRef *result)
{
    NodeRef first = 0;
    NodeRef last = 0;
    int has_item = 0;
    ++state->cursor;
    for (;;) {
        NodeRef item;
        NodeRef pair;
        SlispStatus status;
        skip_space(state);
        if (state->source[state->cursor] == ')') {
            ++state->cursor;
            *result = first;
            return SLISP_OK;
        }
        if (state->source[state->cursor] == '\0') {
            return SLISP_ERR_SYNTAX;
        }
        status = parse_expression(state, &item);
        if (status != SLISP_OK) {
            return status;
        }
        status = make_pair(state, item, 0, &pair);
        if (status != SLISP_OK) {
            return status;
        }
        if (!has_item) {
            first = pair;
            has_item = 1;
        } else {
            state->nodes[last].value.pair.cdr = pair;
        }
        last = pair;
    }
}

static SlispStatus parse_atom(State *state, NodeRef *result)
{
    size_t start = state->cursor;
    size_t len;
    int negative = 0;
    int numeric = 1;
    int64_t value = 0;
    size_t index;
    SlispStatus status;
    while (!is_delimiter(state->source[state->cursor])) {
        ++state->cursor;
    }
    len = state->cursor - start;
    if (len == 0 || len >= SLISP_MAX_SYMBOL_BYTES) {
        return len == 0 ? SLISP_ERR_SYNTAX : SLISP_ERR_LIMIT;
    }
    if (state->source[start] == '-' && len > 1) {
        negative = 1;
        index = 1;
    } else {
        index = 0;
    }
    if (index == len) {
        numeric = 0;
    }
    for (; index < len; ++index) {
        if (!is_digit(state->source[start + index])) {
            numeric = 0;
            break;
        }
        if (value > (INT64_MAX - 9) / 10) {
            return SLISP_ERR_LIMIT;
        }
        value = value * 10 + (state->source[start + index] - '0');
    }
    if (numeric) {
        status = allocate_node(state, NODE_INT, result);
        if (status != SLISP_OK) {
            return status;
        }
        state->nodes[*result].value.integer = negative ? -value : value;
        return SLISP_OK;
    }
    status = allocate_node(state, NODE_SYMBOL, result);
    if (status != SLISP_OK) {
        return status;
    }
    for (index = 0; index < len; ++index) {
        state->nodes[*result].value.symbol[index] = state->source[start + index];
    }
    state->nodes[*result].value.symbol[len] = '\0';
    return SLISP_OK;
}

static SlispStatus parse_expression(State *state, NodeRef *result)
{
    skip_space(state);
    if (state->source[state->cursor] == '(') {
        return parse_list(state, result);
    }
    if (state->source[state->cursor] == ')' || state->source[state->cursor] == '\0') {
        return SLISP_ERR_SYNTAX;
    }
    return parse_atom(state, result);
}

static int symbol_is(State *state, NodeRef node, const char *name)
{
    return state->nodes[node].kind == NODE_SYMBOL
        && text_equal(state->nodes[node].value.symbol, name);
}

static SlispStatus list_take(State *state, NodeRef list, NodeRef *item, NodeRef *rest)
{
    if (list == 0 || state->nodes[list].kind != NODE_PAIR) {
        return SLISP_ERR_ARITY;
    }
    *item = state->nodes[list].value.pair.car;
    *rest = state->nodes[list].value.pair.cdr;
    return SLISP_OK;
}

static SlispStatus list_exact(State *state, NodeRef list, size_t count, NodeRef *items)
{
    size_t index;
    for (index = 0; index < count; ++index) {
        SlispStatus status = list_take(state, list, &items[index], &list);
        if (status != SLISP_OK) {
            return status;
        }
    }
    return list == 0 ? SLISP_OK : SLISP_ERR_ARITY;
}

static SlispStatus bind(State *state, EnvRef parent, const char *name, NodeRef value, EnvRef *result)
{
    Binding *binding;
    if (state->binding_count >= SLISP_MAX_BINDINGS) {
        return SLISP_ERR_LIMIT;
    }
    ++state->binding_count;
    binding = &state->bindings[state->binding_count - 1];
    text_copy(binding->name, name, sizeof(binding->name));
    binding->value = value;
    binding->parent = parent;
    *result = state->binding_count;
    return SLISP_OK;
}

static SlispStatus lookup(State *state, EnvRef environment, const char *name, NodeRef *result)
{
    while (environment != 0) {
        Binding *binding = &state->bindings[environment - 1];
        if (text_equal(binding->name, name)) {
            *result = binding->value;
            return SLISP_OK;
        }
        environment = binding->parent;
    }
    return SLISP_ERR_UNBOUND;
}

static SlispStatus evaluate(State *state, NodeRef expression, EnvRef environment, unsigned depth, NodeRef *result);

static SlispStatus evaluate_sequence(State *state, NodeRef forms, EnvRef environment, unsigned depth, NodeRef *result)
{
    if (forms == 0) {
        *result = 0;
        return SLISP_OK;
    }
    for (;;) {
        NodeRef form;
        SlispStatus status = list_take(state, forms, &form, &forms);
        if (status != SLISP_OK) {
            return status;
        }
        status = evaluate(state, form, environment, depth + 1, result);
        if (status != SLISP_OK || forms == 0) {
            return status;
        }
    }
}

static SlispStatus evaluate_integer_args(
    State *state,
    NodeRef forms,
    EnvRef environment,
    unsigned depth,
    int64_t *left,
    int64_t *right)
{
    NodeRef items[2];
    NodeRef values[2];
    SlispStatus status = list_exact(state, forms, 2, items);
    if (status != SLISP_OK) {
        return status;
    }
    status = evaluate(state, items[0], environment, depth + 1, &values[0]);
    if (status != SLISP_OK) {
        return status;
    }
    status = evaluate(state, items[1], environment, depth + 1, &values[1]);
    if (status != SLISP_OK) {
        return status;
    }
    if (state->nodes[values[0]].kind != NODE_INT || state->nodes[values[1]].kind != NODE_INT) {
        return SLISP_ERR_TYPE;
    }
    *left = state->nodes[values[0]].value.integer;
    *right = state->nodes[values[1]].value.integer;
    return SLISP_OK;
}

static SlispStatus evaluate_builtin(
    State *state,
    const char *name,
    NodeRef arguments,
    EnvRef environment,
    unsigned depth,
    NodeRef *result)
{
    int64_t left;
    int64_t right;
    int64_t value;
    SlispStatus status = evaluate_integer_args(state, arguments, environment, depth, &left, &right);
    if (status != SLISP_OK) {
        return status;
    }
    if (text_equal(name, "+")) {
        value = left + right;
    } else if (text_equal(name, "-")) {
        value = left - right;
    } else if (text_equal(name, "*")) {
        value = left * right;
    } else if (text_equal(name, "/")) {
        if (right == 0) {
            return SLISP_ERR_DIV_ZERO;
        }
        value = left / right;
    } else {
        SlispStatus allocation = allocate_node(state, NODE_BOOL, result);
        if (allocation != SLISP_OK) {
            return allocation;
        }
        state->nodes[*result].value.boolean = (left == right);
        return SLISP_OK;
    }
    status = allocate_node(state, NODE_INT, result);
    if (status != SLISP_OK) {
        return status;
    }
    state->nodes[*result].value.integer = value;
    return SLISP_OK;
}

static SlispStatus apply_closure(
    State *state,
    NodeRef closure,
    NodeRef arguments,
    EnvRef caller,
    unsigned depth,
    NodeRef *result)
{
    NodeRef parameters = state->nodes[closure].value.closure.parameters;
    EnvRef environment = state->nodes[closure].value.closure.environment;
    while (parameters != 0 || arguments != 0) {
        NodeRef parameter;
        NodeRef argument;
        NodeRef value;
        SlispStatus status;
        if (parameters == 0 || arguments == 0) {
            return SLISP_ERR_ARITY;
        }
        status = list_take(state, parameters, &parameter, &parameters);
        if (status != SLISP_OK || state->nodes[parameter].kind != NODE_SYMBOL) {
            return SLISP_ERR_TYPE;
        }
        status = list_take(state, arguments, &argument, &arguments);
        if (status != SLISP_OK) {
            return status;
        }
        status = evaluate(state, argument, caller, depth + 1, &value);
        if (status != SLISP_OK) {
            return status;
        }
        status = bind(state, environment, state->nodes[parameter].value.symbol, value, &environment);
        if (status != SLISP_OK) {
            return status;
        }
    }
    return evaluate_sequence(
        state,
        state->nodes[closure].value.closure.body,
        environment,
        depth + 1,
        result);
}

static SlispStatus evaluate_list(
    State *state,
    NodeRef expression,
    EnvRef environment,
    unsigned depth,
    NodeRef *result)
{
    NodeRef head;
    NodeRef arguments;
    SlispStatus status = list_take(state, expression, &head, &arguments);
    if (status != SLISP_OK) {
        return status;
    }
    if (state->nodes[head].kind == NODE_SYMBOL) {
        const char *name = state->nodes[head].value.symbol;
        if (text_equal(name, "quote")) {
            NodeRef items[1];
            status = list_exact(state, arguments, 1, items);
            if (status == SLISP_OK) {
                *result = items[0];
            }
            return status;
        }
        if (text_equal(name, "if")) {
            NodeRef items[3];
            NodeRef condition;
            status = list_exact(state, arguments, 3, items);
            if (status != SLISP_OK) {
                return status;
            }
            status = evaluate(state, items[0], environment, depth + 1, &condition);
            if (status != SLISP_OK) {
                return status;
            }
            return evaluate(
                state,
                state->nodes[condition].kind == NODE_BOOL && !state->nodes[condition].value.boolean
                    ? items[2]
                    : items[1],
                environment,
                depth + 1,
                result);
        }
        if (text_equal(name, "fn")) {
            NodeRef parameters;
            NodeRef body;
            if (arguments == 0) {
                return SLISP_ERR_ARITY;
            }
            status = list_take(state, arguments, &parameters, &body);
            if (status != SLISP_OK || body == 0) {
                return SLISP_ERR_ARITY;
            }
            for (NodeRef cursor = parameters; cursor != 0;) {
                NodeRef parameter;
                status = list_take(state, cursor, &parameter, &cursor);
                if (status != SLISP_OK || state->nodes[parameter].kind != NODE_SYMBOL) {
                    return SLISP_ERR_TYPE;
                }
            }
            status = allocate_node(state, NODE_CLOSURE, result);
            if (status != SLISP_OK) {
                return status;
            }
            state->nodes[*result].value.closure.parameters = parameters;
            state->nodes[*result].value.closure.body = body;
            state->nodes[*result].value.closure.environment = environment;
            return SLISP_OK;
        }
        if (text_equal(name, "define")) {
            NodeRef items[2];
            NodeRef value;
            status = list_exact(state, arguments, 2, items);
            if (status != SLISP_OK) {
                return status;
            }
            if (state->nodes[items[0]].kind != NODE_SYMBOL) {
                return SLISP_ERR_TYPE;
            }
            status = evaluate(state, items[1], environment, depth + 1, &value);
            if (status != SLISP_OK) {
                return status;
            }
            status = bind(
                state,
                state->global_environment,
                state->nodes[items[0]].value.symbol,
                value,
                &state->global_environment);
            if (status == SLISP_OK) {
                *result = value;
            }
            return status;
        }
        if (text_equal(name, "let")) {
            NodeRef bindings;
            NodeRef body;
            EnvRef extended = environment;
            status = list_take(state, arguments, &bindings, &body);
            if (status != SLISP_OK || body == 0) {
                return SLISP_ERR_ARITY;
            }
            while (bindings != 0) {
                NodeRef binding_form;
                NodeRef binding_items[2];
                NodeRef value;
                status = list_take(state, bindings, &binding_form, &bindings);
                if (status != SLISP_OK) {
                    return status;
                }
                status = list_exact(state, binding_form, 2, binding_items);
                if (status != SLISP_OK || state->nodes[binding_items[0]].kind != NODE_SYMBOL) {
                    return SLISP_ERR_TYPE;
                }
                status = evaluate(state, binding_items[1], environment, depth + 1, &value);
                if (status != SLISP_OK) {
                    return status;
                }
                status = bind(
                    state,
                    extended,
                    state->nodes[binding_items[0]].value.symbol,
                    value,
                    &extended);
                if (status != SLISP_OK) {
                    return status;
                }
            }
            return evaluate_sequence(state, body, extended, depth + 1, result);
        }
        if (text_equal(name, "do")) {
            return evaluate_sequence(state, arguments, environment, depth + 1, result);
        }
        if (text_equal(name, "+") || text_equal(name, "-") || text_equal(name, "*")
            || text_equal(name, "/") || text_equal(name, "=")) {
            return evaluate_builtin(state, name, arguments, environment, depth + 1, result);
        }
    }
    status = evaluate(state, head, environment, depth + 1, &head);
    if (status != SLISP_OK) {
        return status;
    }
    if (state->nodes[head].kind != NODE_CLOSURE) {
        return SLISP_ERR_TYPE;
    }
    return apply_closure(state, head, arguments, environment, depth + 1, result);
}

static SlispStatus evaluate(State *state, NodeRef expression, EnvRef environment, unsigned depth, NodeRef *result)
{
    if (depth > SLISP_MAX_EVAL_DEPTH) {
        return SLISP_ERR_LIMIT;
    }
    switch (state->nodes[expression].kind) {
    case NODE_NIL:
    case NODE_BOOL:
    case NODE_INT:
    case NODE_CLOSURE:
        *result = expression;
        return SLISP_OK;
    case NODE_SYMBOL:
        if (symbol_is(state, expression, "nil")) {
            *result = 0;
            return SLISP_OK;
        }
        if (symbol_is(state, expression, "true") || symbol_is(state, expression, "false")) {
            SlispStatus status = allocate_node(state, NODE_BOOL, result);
            if (status != SLISP_OK) {
                return status;
            }
            state->nodes[*result].value.boolean = symbol_is(state, expression, "true");
            return SLISP_OK;
        }
        return lookup(state, environment, state->nodes[expression].value.symbol, result);
    case NODE_PAIR:
        return evaluate_list(state, expression, environment, depth + 1, result);
    default:
        return SLISP_ERR_TYPE;
    }
}

static SlispStatus append_text(char *output, size_t capacity, size_t *used, const char *text)
{
    size_t len = text_len(text);
    size_t index;
    if (*used + len + 1 > capacity) {
        return SLISP_ERR_LIMIT;
    }
    for (index = 0; index < len; ++index) {
        output[*used + index] = text[index];
    }
    *used += len;
    output[*used] = '\0';
    return SLISP_OK;
}

static SlispStatus render_value(State *state, NodeRef value, char *output, size_t capacity, size_t *used)
{
    char digits[32];
    size_t count = 0;
    uint64_t magnitude;
    SlispStatus status;
    if (value == 0 || state->nodes[value].kind == NODE_NIL) {
        return append_text(output, capacity, used, "nil");
    }
    switch (state->nodes[value].kind) {
    case NODE_BOOL:
        return append_text(
            output,
            capacity,
            used,
            state->nodes[value].value.boolean ? "true" : "false");
    case NODE_INT:
        magnitude = state->nodes[value].value.integer < 0
            ? (uint64_t)(-(state->nodes[value].value.integer + 1)) + 1
            : (uint64_t)state->nodes[value].value.integer;
        do {
            digits[count++] = (char)('0' + magnitude % 10);
            magnitude /= 10;
        } while (magnitude != 0);
        if (state->nodes[value].value.integer < 0) {
            status = append_text(output, capacity, used, "-");
            if (status != SLISP_OK) {
                return status;
            }
        }
        while (count != 0) {
            char character[2] = { digits[--count], '\0' };
            status = append_text(output, capacity, used, character);
            if (status != SLISP_OK) {
                return status;
            }
        }
        return SLISP_OK;
    case NODE_SYMBOL:
        return append_text(output, capacity, used, state->nodes[value].value.symbol);
    case NODE_PAIR: {
        NodeRef cursor = value;
        status = append_text(output, capacity, used, "(");
        if (status != SLISP_OK) {
            return status;
        }
        while (cursor != 0) {
            NodeRef item;
            NodeRef rest;
            status = list_take(state, cursor, &item, &rest);
            if (status != SLISP_OK) {
                return status;
            }
            status = render_value(state, item, output, capacity, used);
            if (status != SLISP_OK) {
                return status;
            }
            cursor = rest;
            if (cursor != 0) {
                status = append_text(output, capacity, used, " ");
                if (status != SLISP_OK) {
                    return status;
                }
            }
        }
        return append_text(output, capacity, used, ")");
    }
    case NODE_CLOSURE:
        return append_text(output, capacity, used, "<fn>");
    default:
        return SLISP_ERR_TYPE;
    }
}

static void clear_state(State *state)
{
    size_t state_byte;
    for (state_byte = 0; state_byte < sizeof(*state); ++state_byte) {
        ((unsigned char *)state)[state_byte] = 0;
    }
    state->node_count = 1;
    state->nodes[0].kind = NODE_NIL;
}

static SlispStatus run_in_state(
    State *state,
    EnvRef environment,
    const char *source,
    char *output,
    size_t output_capacity)
{
    NodeRef expression;
    NodeRef value;
    size_t used = 0;
    SlispStatus status;
    if (output_capacity == 0) {
        return SLISP_ERR_LIMIT;
    }
    output[0] = '\0';
    state->source = source;
    state->cursor = 0;
    status = parse_expression(state, &expression);
    if (status != SLISP_OK) {
        return status;
    }
    skip_space(state);
    if (state->source[state->cursor] != '\0') {
        return SLISP_ERR_SYNTAX;
    }
    status = evaluate(state, expression, environment, 0, &value);
    if (status != SLISP_OK) {
        return status;
    }
    return render_value(state, value, output, output_capacity, &used);
}

SlispStatus slisp_run(const char *source, char *output, size_t output_capacity)
{
    State state;
    clear_state(&state);
    return run_in_state(&state, state.global_environment, source, output, output_capacity);
}

static State session_state;
static int session_initialized;

void slisp_session_reset(void)
{
    clear_state(&session_state);
    session_initialized = 1;
}

SlispStatus slisp_session_run(const char *source, char *output, size_t output_capacity)
{
    if (!session_initialized) {
        slisp_session_reset();
    }
    return run_in_state(
        &session_state,
        session_state.global_environment,
        source,
        output,
        output_capacity);
}

const char *slisp_status_name(SlispStatus status)
{
    switch (status) {
    case SLISP_OK:
        return "ok";
    case SLISP_ERR_SYNTAX:
        return "syntax";
    case SLISP_ERR_LIMIT:
        return "limit";
    case SLISP_ERR_UNBOUND:
        return "unbound";
    case SLISP_ERR_TYPE:
        return "type";
    case SLISP_ERR_ARITY:
        return "arity";
    case SLISP_ERR_DIV_ZERO:
        return "divide-by-zero";
    default:
        return "unknown";
    }
}
