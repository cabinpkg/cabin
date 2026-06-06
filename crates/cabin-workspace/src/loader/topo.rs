use crate::error::WorkspaceError;
use crate::graph::WorkspacePackage;

pub(super) fn topo_sort(packages: &[WorkspacePackage]) -> Result<Vec<usize>, WorkspaceError> {
    #[derive(Clone, Copy)]
    enum Color {
        Visiting,
        Done,
    }

    fn visit(
        node: usize,
        packages: &[WorkspacePackage],
        state: &mut Vec<Option<Color>>,
        path: &mut Vec<usize>,
        order: &mut Vec<usize>,
    ) -> Result<(), WorkspaceError> {
        match state[node] {
            Some(Color::Done) => return Ok(()),
            Some(Color::Visiting) => {
                let start = path.iter().position(|n| *n == node).unwrap_or(0);
                let mut cycle: Vec<String> = path[start..]
                    .iter()
                    .map(|i| packages[*i].package.name.as_str().to_owned())
                    .collect();
                cycle.push(packages[node].package.name.as_str().to_owned());
                return Err(WorkspaceError::PackageDependencyCycle(cycle));
            }
            None => {}
        }
        state[node] = Some(Color::Visiting);
        path.push(node);
        for edge in &packages[node].deps {
            visit(edge.index, packages, state, path, order)?;
        }
        path.pop();
        state[node] = Some(Color::Done);
        order.push(node);
        Ok(())
    }

    let mut state: Vec<Option<Color>> = vec![None; packages.len()];
    let mut order = Vec::with_capacity(packages.len());
    let mut path = Vec::new();

    // Visit packages in their original (insertion) order so the output is
    // deterministic for inputs that don't fully order themselves.
    for i in 0..packages.len() {
        if state[i].is_none() {
            visit(i, packages, &mut state, &mut path, &mut order)?;
        }
    }
    Ok(order)
}
