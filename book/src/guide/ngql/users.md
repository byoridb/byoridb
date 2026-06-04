# User Management

ByoriDB provides role-based access control (RBAC) for managing users and permissions.

## Users

### CREATE USER

Create a new user with a password and optional role:

```sql
CREATE USER <username> WITH PASSWORD '<password>' [ROLE <role>];
```

**Examples:**

```sql
-- Create user without specifying a role (no roles assigned by default)
CREATE USER alice WITH PASSWORD 'secure123';

-- Create user with specific role
CREATE USER bob WITH PASSWORD 'pass456' ROLE ADMIN;

-- Create user if not exists
CREATE USER IF NOT EXISTS charlie WITH PASSWORD 'mypass';
```

### ALTER USER

Change a user's password:

```sql
ALTER USER <username> WITH PASSWORD '<new_password>';
```

**Examples:**

```sql
ALTER USER alice WITH PASSWORD 'newsecure456';
```

### DROP USER

Delete a user:

```sql
DROP USER <username>;
DROP USER IF EXISTS <username>;
```

**Examples:**

```sql
DROP USER alice;
DROP USER IF EXISTS bob;
```

> **Note:** The `root` user cannot be deleted.

> **Note:** SHOW USERS is not yet implemented.

## Roles

ByoriDB has five built-in roles with different permission levels:

| Role    | Permissions                              | Description           |
|---------|------------------------------------------|-----------------------|
| GOD     | All                                      | Superuser (root only) |
| ADMIN   | Read, Write, Create, Delete, Alter, Drop | Full administrator    |
| DBA     | Read, Write, Create, Alter               | Database administrator|
| USER    | Read, Write                              | Standard user         |
| GUEST   | Read                                     | Read-only access      |

### Permission Types

- **Read**: Query data (FETCH, GO, MATCH, LOOKUP)
- **Write**: Modify data (INSERT, UPDATE, DELETE vertex/edge)
- **Create**: Create schemas (CREATE SPACE/TAG/EDGE)
- **Alter**: Modify schemas (ALTER TAG/EDGE)
- **Drop**: Drop schemas (DROP SPACE/TAG/EDGE)

### GRANT ROLE

Assign a role to a user:

```sql
GRANT ROLE <role> TO <username>;
```

**Examples:**

```sql
GRANT ROLE ADMIN TO alice;
GRANT ROLE USER TO bob;
GRANT ROLE GUEST TO viewer;
```

### REVOKE ROLE

Remove a role from a user:

```sql
REVOKE ROLE <role> FROM <username>;
```

**Examples:**

```sql
REVOKE ROLE ADMIN FROM alice;
REVOKE ROLE USER FROM bob;
```

> **Note:** SHOW ROLES is not yet implemented.

## Default User

ByoriDB creates a default superuser on first startup:

- **Username:** `root`
- **Password:** The value of `BYORIDB_ROOT_PASSWORD`, or a generated password logged once at startup
- **Role:** `GOD`

> **Security Warning:** Set `BYORIDB_ROOT_PASSWORD` from a secret manager or protected environment before production startup.

```sql
ALTER USER root WITH PASSWORD 'your_secure_password';
```

## Best Practices

1. **Set the root password explicitly** - Provide `BYORIDB_ROOT_PASSWORD` before startup
2. **Principle of least privilege** - Grant users only the minimum permissions they need
3. **Use ADMIN sparingly** - Reserve ADMIN role for database administrators
4. **Use GUEST for read-only access** - Perfect for reporting and analytics users
5. **Regular audits** - Periodically review user accounts and roles to ensure appropriate permissions
