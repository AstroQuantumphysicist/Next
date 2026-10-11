//! Role-specific namespace authority decision. This helper grants no object,
//! chooses no path and authenticates no principal. The caller must validate the
//! returned permissions/rights against its independently authenticated grant.
use next_abi::{Rights, Status};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Launcher,
    Namespace,
    Other,
}
pub fn decision(
    role: Role,
    available: u64,
    declared_permissions: u64,
    write: bool,
) -> Result<(u64, u64), Status> {
    if !Rights(available).is_valid() {
        return Err(Status::AccessDenied);
    }
    let (permissions, required) = match role {
        Role::Launcher => (0, Rights::EXECUTE.0),
        Role::Namespace if write => (3, Rights::READ.union(Rights::WRITE).0),
        Role::Namespace => (1, Rights::READ.0),
        Role::Other => return Err(Status::AccessDenied),
    };
    if available & required != required || declared_permissions & permissions != permissions {
        return Err(Status::AccessDenied);
    }
    Ok((permissions, required))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_rights_and_declaration_matrix_preserves_role_and_direction() {
        // Every known wire mask, independently crossed with read/write manifest
        // bits, catches EXECUTE/WAIT/DUP substitutions and missing READ on write.
        for rights in 0..=0xff {
            for declared in 0..=7 {
                for write in [false, true] {
                    let namespace = decision(Role::Namespace, rights, declared, write);
                    let required = if write { 3 } else { 1 };
                    if rights & required == required && declared & required == required {
                        assert_eq!(namespace, Ok((required, required)));
                    } else {
                        assert_eq!(namespace, Err(Status::AccessDenied));
                    }
                    let launcher = decision(Role::Launcher, rights, declared, write);
                    assert_eq!(
                        launcher,
                        if rights & Rights::EXECUTE.0 != 0 {
                            Ok((0, Rights::EXECUTE.0))
                        } else {
                            Err(Status::AccessDenied)
                        }
                    );
                    assert_eq!(
                        decision(Role::Other, rights, declared, write),
                        Err(Status::AccessDenied)
                    );
                }
            }
        }
    }
    #[test]
    fn attenuation_cannot_create_namespace_or_launcher_authority() {
        let granted = Rights::READ.union(Rights::WRITE).0;
        for subset in 0..=0xff {
            if subset & !granted != 0 {
                continue;
            }
            assert_eq!(
                decision(Role::Launcher, subset, 3, false),
                Err(Status::AccessDenied)
            );
            assert_eq!(
                decision(Role::Namespace, subset, 3, false).is_ok(),
                subset & 1 != 0
            );
            assert_eq!(
                decision(Role::Namespace, subset, 3, true).is_ok(),
                subset == granted
            );
        }
        assert_eq!(
            decision(Role::Namespace, Rights::WRITE.0, 3, true),
            Err(Status::AccessDenied)
        );
        for bit in 8..64 {
            assert_eq!(
                decision(Role::Namespace, granted | (1 << bit), 3, true),
                Err(Status::AccessDenied)
            );
        }
        assert_eq!(
            decision(Role::Namespace, granted, 1, true),
            Err(Status::AccessDenied)
        );
        assert_eq!(
            decision(Role::Namespace, granted, 2, false),
            Err(Status::AccessDenied)
        );
    }
}
