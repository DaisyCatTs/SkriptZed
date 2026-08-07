//! Call hierarchy.
//!
//! "Who calls this?" and "what does this call?" are the two questions a
//! `references` list cannot answer, because a flat list of call sites loses the
//! thing that matters: which trigger or function each call is *inside*.
//!
//! Skript has no call graph of its own — a function is reachable from anything,
//! and most callers are events rather than other functions. So the caller of a
//! call site is simply the top-level structure that encloses it, whatever that
//! structure happens to be. `on join:` shows up in an incoming-calls tree next
//! to `function payout()`, which is exactly right: both are places the function
//! runs from.
//!
//! Everything here is pure and takes a borrowed [`Workspace`], so it is
//! testable without an LSP session.

use skript_index::{Document, Range, Symbol, SymbolKind, Workspace};

/// One node of the tree: a declaration, and the file it lives in.
pub struct Node<'a> {
    pub document: &'a Document,
    pub symbol: &'a Symbol,
}

/// A node plus the call sites that link it to its parent.
pub struct Call<'a> {
    pub node: Node<'a>,
    /// Ranges in `node.document` — the spec anchors both directions to the
    /// *other* end of the edge, so these are always ranges in the file the
    /// returned node lives in.
    pub ranges: Vec<Range>,
}

/// The declaration a call hierarchy should start from, if the cursor is on one.
///
/// Accepts both a declaration and a call site, because "show me who calls this"
/// is asked from a call just as often as from the signature.
pub fn prepare<'a>(
    workspace: &'a Workspace,
    document: &'a Document,
    position: skript_index::Position,
) -> Vec<Node<'a>> {
    let symbols = document.symbols();

    let (kind, name) = match symbols.declaration_at(position) {
        Some(symbol) if symbol.kind.is_function() => (symbol.kind, symbol.name.clone()),
        // A parameter or a variable is not callable; asking from one should do
        // nothing rather than silently answer about the enclosing function.
        Some(_) => return Vec::new(),
        None => match symbols.reference_at(position) {
            Some(reference) if reference.kind.is_function() => {
                (reference.kind, reference.name.clone())
            }
            _ => return Vec::new(),
        },
    };

    workspace
        .definitions(kind, &name, document.uri())
        .into_iter()
        .map(|(document, symbol)| Node { document, symbol })
        .collect()
}

/// Every place `node` is called from, grouped by the structure containing the
/// call.
pub fn incoming<'a>(workspace: &'a Workspace, node: &Node<'a>) -> Vec<Call<'a>> {
    let mut out: Vec<Call<'a>> = Vec::new();

    for (document, reference) in
        workspace.references(node.symbol.kind, &node.symbol.name, node.document.uri())
    {
        // A call at the top level of a file with no enclosing structure is not
        // reachable code in Skript, but it parses; skipping it is better than
        // inventing a caller for it.
        let Some(caller) = enclosing(document, reference.range) else {
            continue;
        };
        push(&mut out, document, caller, reference.range);
    }

    out
}

/// Every function `node` calls, grouped by callee.
pub fn outgoing<'a>(workspace: &'a Workspace, node: &Node<'a>) -> Vec<Call<'a>> {
    let mut out: Vec<Call<'a>> = Vec::new();

    for reference in &node.document.symbols().references {
        if !reference.kind.is_function() || !encloses(node.symbol.range, reference.range) {
            continue;
        }
        for (document, symbol) in
            workspace.definitions(reference.kind, &reference.name, node.document.uri())
        {
            // The range belongs to the caller's file — it is where the call is
            // written, not where the callee is declared.
            push(&mut out, document, symbol, reference.range);
        }
    }

    out
}

/// Adds a call site, merging into an existing node rather than repeating it.
fn push<'a>(out: &mut Vec<Call<'a>>, document: &'a Document, symbol: &'a Symbol, range: Range) {
    if let Some(existing) = out.iter_mut().find(|call| {
        call.node.document.uri() == document.uri() && call.node.symbol.range == symbol.range
    }) {
        existing.ranges.push(range);
        return;
    }
    out.push(Call {
        node: Node { document, symbol },
        ranges: vec![range],
    });
}

/// The top-level structure containing `range`.
///
/// Only the outermost level is considered. A call inside a nested `if` belongs
/// to the trigger, not to the `if` — nesting is control flow, not a caller.
fn enclosing(document: &Document, range: Range) -> Option<&Symbol> {
    document
        .symbols()
        .symbols
        .iter()
        .find(|symbol| encloses(symbol.range, range))
}

fn encloses(outer: Range, inner: Range) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// How a node should read in the tree.
///
/// A `local function` is labelled as such, because whether a call can cross
/// files is the first thing you want to know when reading a call graph.
pub fn detail(symbol: &Symbol) -> Option<String> {
    match symbol.kind {
        SymbolKind::LocalFunction => Some(format!("local · {}", symbol.detail).trim_end().into()),
        _ => (!symbol.detail.is_empty()).then(|| symbol.detail.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skript_index::Position;

    fn workspace(files: &[(&str, &str)]) -> Workspace {
        let mut workspace = Workspace::new();
        for (uri, text) in files {
            workspace.open(*uri, *text);
        }
        workspace
    }

    /// The position of `name`'s first occurrence, as a cursor would land on it.
    fn at(text: &str, needle: &str) -> Position {
        let offset = text.find(needle).expect("the needle is in the source");
        let line = text[..offset].matches('\n').count() as u32;
        let column = offset - text[..offset].rfind('\n').map(|at| at + 1).unwrap_or(0);
        Position::new(line, column as u32)
    }

    #[test]
    fn an_event_counts_as_a_caller() {
        let text = "function payout(p: player):\n\tstop\n\non join:\n\tpayout(player)\n";
        let workspace = workspace(&[("file:///a.sk", text)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        let roots = prepare(&workspace, document, at(text, "payout"));
        assert_eq!(roots.len(), 1);

        let callers = incoming(&workspace, &roots[0]);
        assert_eq!(callers.len(), 1);
        // Not a function, but unambiguously the place the call runs from.
        assert_eq!(callers[0].node.symbol.kind, SymbolKind::Event);
        assert_eq!(callers[0].ranges.len(), 1);
    }

    #[test]
    fn two_calls_in_one_trigger_are_one_caller() {
        let text = "function a():\n\tstop\n\non join:\n\ta()\n\ta()\n";
        let workspace = workspace(&[("file:///a.sk", text)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        let roots = prepare(&workspace, document, at(text, "a"));
        let callers = incoming(&workspace, &roots[0]);
        assert_eq!(callers.len(), 1, "one node, not one per call site");
        assert_eq!(callers[0].ranges.len(), 2);
    }

    #[test]
    fn calls_are_found_across_files() {
        let a = "function payout():\n\tstop\n";
        let b = "on join:\n\tpayout()\n";
        let workspace = workspace(&[("file:///a.sk", a), ("file:///b.sk", b)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        let roots = prepare(&workspace, document, at(a, "payout"));
        let callers = incoming(&workspace, &roots[0]);
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].node.document.uri(), "file:///b.sk");
    }

    #[test]
    fn a_local_function_is_not_called_from_another_file() {
        let a = "local function secret():\n\tstop\n";
        let b = "on join:\n\tsecret()\n";
        let workspace = workspace(&[("file:///a.sk", a), ("file:///b.sk", b)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        let roots = prepare(&workspace, document, at(a, "secret"));
        assert!(
            incoming(&workspace, &roots[0]).is_empty(),
            "a local function's callers cannot live in another file"
        );
    }

    #[test]
    fn outgoing_lists_what_the_body_calls() {
        let text = "function a():\n\tb()\n\nfunction b():\n\tstop\n";
        let workspace = workspace(&[("file:///a.sk", text)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        let roots = prepare(&workspace, document, at(text, "a"));
        let calls = outgoing(&workspace, &roots[0]);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].node.symbol.name, "b");
    }

    #[test]
    fn outgoing_stops_at_the_function_body() {
        // `c()` is in a sibling function and must not be attributed to `a`.
        let text = "function a():\n\tb()\n\nfunction b():\n\tc()\n\nfunction c():\n\tstop\n";
        let workspace = workspace(&[("file:///a.sk", text)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        let roots = prepare(&workspace, document, at(text, "a"));
        let names: Vec<_> = outgoing(&workspace, &roots[0])
            .into_iter()
            .map(|call| call.node.symbol.name.clone())
            .collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn a_variable_is_not_callable() {
        let text = "on join:\n\tset {_x} to 1\n";
        let workspace = workspace(&[("file:///a.sk", text)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        assert!(prepare(&workspace, document, at(text, "{_x}")).is_empty());
    }

    #[test]
    fn the_hierarchy_starts_from_a_call_site_too() {
        let text = "function payout():\n\tstop\n\non join:\n\tpayout()\n";
        let workspace = workspace(&[("file:///a.sk", text)]);
        let document = workspace.get("file:///a.sk").expect("opened");

        // Line 4 column 1 is the call, not the declaration on line 0.
        let roots = prepare(&workspace, document, Position::new(4, 1));
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].symbol.name, "payout");
    }
}
