//! Works around naga's HLSL writer (wgpu's DX12 backend) mistranslating a
//! common loop shape.
//!
//! HLSL has no `continuing` block, so the writer moves it to the top of
//! `while (true)` behind a `loop_init` guard. Values the body loaded from a
//! local variable (`let a0 = a;`) are then not declared yet where the
//! continuing block uses them, and the writer reads the variable again
//! instead: after a store to it earlier in the same continuing block, that
//! is the new value. DXC turns `for (...) { v += a * x; a *= 0.5; }` into
//! exactly this (`a = a0 * 0.5; v = v0 + a0 * x;`), so on DX12 every term
//! used the halved `a` (Neon Sunset's terrain came out at half height).
//!
//! The fix keeps the meaning on every backend: stores in the continuing
//! block to such variables go to temporaries first and reach the variables
//! only at the end of the block, so every value computed in the block reads
//! the old ones. It applies only where that is provably the same program;
//! other loops are left alone.

use std::collections::HashSet;

use naga::{
    Block, Expression, Function, Handle, LocalVariable, Module, Range, SampleLevel, Span, Statement,
};

/// Applies the fix to every function of `module`.
pub(crate) fn defer_continuing_stores(module: &mut Module) {
    for (_, function) in module.functions.iter_mut() {
        fix_function(function);
    }
    for entry in &mut module.entry_points {
        fix_function(&mut entry.function);
    }
}

fn fix_function(function: &mut Function) {
    let mut body = std::mem::take(&mut function.body);
    fix_block(&mut body, function);
    function.body = body;
}

fn fix_block(block: &mut Block, function: &mut Function) {
    for statement in block.iter_mut() {
        match statement {
            Statement::Block(inner) => fix_block(inner, function),
            Statement::If { accept, reject, .. } => {
                fix_block(accept, function);
                fix_block(reject, function);
            }
            Statement::Switch { cases, .. } => {
                for case in cases {
                    fix_block(&mut case.body, function);
                }
            }
            Statement::Loop {
                body,
                continuing,
                break_if,
            } => {
                fix_block(body, function);
                fix_block(continuing, function);
                fix_loop(body, continuing, *break_if, function);
            }
            _ => {}
        }
    }
}

/// The local variable `pointer` names directly, if any.
fn local(function: &Function, pointer: Handle<Expression>) -> Option<Handle<LocalVariable>> {
    match function.expressions[pointer] {
        Expression::LocalVariable(v) => Some(v),
        _ => None,
    }
}

fn fix_loop(
    body: &Block,
    continuing: &mut Block,
    break_if: Option<Handle<Expression>>,
    function: &mut Function,
) {
    if continuing.is_empty() {
        return;
    }
    // Loads the body emits at its top level, by variable.
    let mut body_loads: Vec<(Handle<Expression>, Handle<LocalVariable>)> = Vec::new();
    for statement in body.iter() {
        if let Statement::Emit(range) = statement {
            for h in range.clone() {
                if let Expression::Load { pointer } = function.expressions[h]
                    && let Some(v) = local(function, pointer)
                {
                    body_loads.push((h, v));
                }
            }
        }
    }
    if body_loads.is_empty() {
        return;
    }
    let mut used = HashSet::new();
    let mut continuing_loads = HashSet::new();
    let mut nested_stores = HashSet::new();
    let mut opaque = false;
    for statement in continuing.iter() {
        scan_statement(
            statement,
            function,
            true,
            &mut used,
            &mut continuing_loads,
            &mut nested_stores,
            &mut opaque,
        );
    }
    if opaque {
        return;
    }
    if let Some(h) = break_if {
        used.insert(h);
    }
    let mut body_stores = HashSet::new();
    for statement in body.iter() {
        stores_in(statement, function, &mut body_stores);
    }
    let top_stores: HashSet<Handle<LocalVariable>> = continuing
        .iter()
        .filter_map(|s| match s {
            Statement::Store { pointer, .. } => local(function, *pointer),
            _ => None,
        })
        .collect();
    let read_by_break = |v: Handle<LocalVariable>| {
        break_if.is_some_and(|b| body_loads.iter().any(|&(h, lv)| lv == v && h == b))
    };
    let targets: HashSet<Handle<LocalVariable>> = top_stores
        .into_iter()
        .filter(|v| {
            body_loads
                .iter()
                .any(|&(h, lv)| lv == *v && used.contains(&h))
                && !body_stores.contains(v)
                && !continuing_loads.contains(v)
                && !nested_stores.contains(v)
                && !read_by_break(*v)
        })
        .collect();
    if targets.is_empty() {
        return;
    }

    // Stores to targets go to temporaries; the variables take them at the end.
    let mut temps: Vec<(
        Handle<LocalVariable>,
        Handle<Expression>,
        Handle<Expression>,
    )> = Vec::new();
    let old = std::mem::take(continuing);
    for (statement, span) in old.span_into_iter() {
        match statement {
            Statement::Store { pointer, value }
                if local(function, pointer).is_some_and(|v| targets.contains(&v)) =>
            {
                let v = local(function, pointer).unwrap_or_else(|| unreachable!());
                let temp_pointer = match temps.iter().find(|t| t.0 == v) {
                    Some(t) => t.2,
                    None => {
                        let var = &function.local_variables[v];
                        let temp = function.local_variables.append(
                            LocalVariable {
                                name: var.name.as_ref().map(|n| format!("{n}_next")),
                                ty: var.ty,
                                init: None,
                            },
                            Span::UNDEFINED,
                        );
                        let temp_pointer = function
                            .expressions
                            .append(Expression::LocalVariable(temp), Span::UNDEFINED);
                        temps.push((v, pointer, temp_pointer));
                        temp_pointer
                    }
                };
                continuing.push(
                    Statement::Store {
                        pointer: temp_pointer,
                        value,
                    },
                    span,
                );
            }
            other => continuing.push(other, span),
        }
    }
    for (_, pointer, temp_pointer) in temps {
        let load = function.expressions.append(
            Expression::Load {
                pointer: temp_pointer,
            },
            Span::UNDEFINED,
        );
        continuing.push(
            Statement::Emit(Range::new_from_bounds(load, load)),
            Span::UNDEFINED,
        );
        continuing.push(
            Statement::Store {
                pointer,
                value: load,
            },
            Span::UNDEFINED,
        );
    }
}

/// Collects what `statement` (part of a continuing block) uses: expression
/// operands, variables it loads, variables stored in nested blocks. Sets
/// `opaque` for statements it cannot see through.
fn scan_statement(
    statement: &Statement,
    function: &Function,
    top: bool,
    used: &mut HashSet<Handle<Expression>>,
    loads: &mut HashSet<Handle<LocalVariable>>,
    nested_stores: &mut HashSet<Handle<LocalVariable>>,
    opaque: &mut bool,
) {
    let scan_block = |block: &Block,
                      used: &mut HashSet<Handle<Expression>>,
                      loads: &mut HashSet<Handle<LocalVariable>>,
                      nested_stores: &mut HashSet<Handle<LocalVariable>>,
                      opaque: &mut bool| {
        for s in block.iter() {
            scan_statement(s, function, false, used, loads, nested_stores, opaque);
        }
    };
    match statement {
        Statement::Emit(range) => {
            for h in range.clone() {
                let expression = &function.expressions[h];
                if let Expression::Load { pointer } = expression
                    && let Some(v) = local(function, *pointer)
                {
                    loads.insert(v);
                }
                if !operands(expression, used) {
                    *opaque = true;
                }
            }
        }
        Statement::Store { pointer, value } => {
            used.insert(*pointer);
            used.insert(*value);
            if !top && let Some(v) = local(function, *pointer) {
                nested_stores.insert(v);
            }
        }
        Statement::Block(b) => scan_block(b, used, loads, nested_stores, opaque),
        Statement::If {
            condition,
            accept,
            reject,
        } => {
            used.insert(*condition);
            scan_block(accept, used, loads, nested_stores, opaque);
            scan_block(reject, used, loads, nested_stores, opaque);
        }
        Statement::Call {
            arguments, result, ..
        } => {
            used.extend(arguments.iter().copied());
            // A pointer argument could read or write a variable anywhere.
            if arguments.iter().any(|a| local(function, *a).is_some()) {
                *opaque = true;
            }
            if let Some(r) = result {
                used.insert(*r);
            }
        }
        Statement::Break | Statement::Continue | Statement::Kill => {}
        // Anything else (barriers, atomics, nested loops, returns…): do not
        // touch this loop.
        _ => *opaque = true,
    }
}

/// Variables `statement` stores to, at any depth.
fn stores_in(
    statement: &Statement,
    function: &Function,
    stores: &mut HashSet<Handle<LocalVariable>>,
) {
    let block = |b: &Block, stores: &mut HashSet<Handle<LocalVariable>>| {
        for s in b.iter() {
            stores_in(s, function, stores);
        }
    };
    match statement {
        Statement::Store { pointer, .. } => {
            if let Some(v) = local(function, *pointer) {
                stores.insert(v);
            }
        }
        Statement::Block(b) => block(b, stores),
        Statement::If { accept, reject, .. } => {
            block(accept, stores);
            block(reject, stores);
        }
        Statement::Switch { cases, .. } => {
            for case in cases {
                block(&case.body, stores);
            }
        }
        Statement::Loop {
            body, continuing, ..
        } => {
            block(body, stores);
            block(continuing, stores);
        }
        Statement::Call { arguments, .. } => {
            // Through a pointer argument.
            for a in arguments {
                if let Some(v) = local(function, *a) {
                    stores.insert(v);
                }
            }
        }
        _ => {}
    }
}

/// Adds `expression`'s operands to `used`; false for kinds this module does
/// not know (the caller then leaves the loop alone).
fn operands(expression: &Expression, used: &mut HashSet<Handle<Expression>>) -> bool {
    let mut add = |h: &Handle<Expression>| {
        used.insert(*h);
    };
    match expression {
        Expression::Literal(_)
        | Expression::Constant(_)
        | Expression::Override(_)
        | Expression::ZeroValue(_)
        | Expression::FunctionArgument(_)
        | Expression::GlobalVariable(_)
        | Expression::LocalVariable(_)
        | Expression::CallResult(_) => {}
        Expression::Compose { components, .. } => components.iter().for_each(add),
        Expression::Access { base, index } => {
            add(base);
            add(index);
        }
        Expression::AccessIndex { base, .. } => add(base),
        Expression::Splat { value, .. } => add(value),
        Expression::Swizzle { vector, .. } => add(vector),
        Expression::Load { pointer } => add(pointer),
        Expression::ImageSample {
            image,
            sampler,
            coordinate,
            array_index,
            offset,
            level,
            depth_ref,
            ..
        } => {
            add(image);
            add(sampler);
            add(coordinate);
            array_index.iter().for_each(&mut add);
            offset.iter().for_each(&mut add);
            depth_ref.iter().for_each(&mut add);
            match level {
                SampleLevel::Exact(h) | SampleLevel::Bias(h) => add(h),
                SampleLevel::Gradient { x, y } => {
                    add(x);
                    add(y);
                }
                SampleLevel::Auto | SampleLevel::Zero => {}
            }
        }
        Expression::ImageLoad {
            image,
            coordinate,
            array_index,
            sample,
            level,
        } => {
            add(image);
            add(coordinate);
            array_index.iter().for_each(&mut add);
            sample.iter().for_each(&mut add);
            level.iter().for_each(&mut add);
        }
        Expression::Unary { expr, .. }
        | Expression::Derivative { expr, .. }
        | Expression::As { expr, .. } => add(expr),
        Expression::Binary { left, right, .. } => {
            add(left);
            add(right);
        }
        Expression::Select {
            condition,
            accept,
            reject,
        } => {
            add(condition);
            add(accept);
            add(reject);
        }
        Expression::Relational { argument, .. } => add(argument),
        Expression::Math {
            arg,
            arg1,
            arg2,
            arg3,
            ..
        } => {
            add(arg);
            arg1.iter().for_each(&mut add);
            arg2.iter().for_each(&mut add);
            arg3.iter().for_each(&mut add);
        }
        Expression::ArrayLength(h) => add(h),
        _ => return false,
    }
    true
}
