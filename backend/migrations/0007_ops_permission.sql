-- Read access to the operator dashboard (/admin, /api/v1/admin/ops/*). Granted to the admin
-- role; grant it to a single user with a `user_permissions` row to give someone the
-- dashboard without user management.
INSERT INTO auth.permissions (name, description) VALUES
    ('ops.view', 'View the operator dashboard: server status, live instances, anomalies');

INSERT INTO auth.role_permissions (role_id, permission_id)
SELECT r.id, p.id FROM auth.roles r, auth.permissions p
WHERE r.name = 'admin' AND p.name = 'ops.view';
