/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.configuration.versioning;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/**
 * Sealed interface for schema migration steps.
 *
 * <p>Each migration transforms a JSON configuration tree from one schema version
 * to the next. Migrations operate on Jackson's {@link ObjectNode} to remain
 * decoupled from concrete record types, enabling version-independent processing.</p>
 *
 * <p>Implementations must be stateless. The {@link #migrate(ObjectNode)} method
 * receives the full config tree and mutates it in place.</p>
 */
public sealed interface SchemaMigrator permits SchemaMigrator.Forward, SchemaMigrator.Rollback {

    /**
     * The source version this migration upgrades from.
     */
    SchemaVersion fromVersion();

    /**
     * The target version this migration produces.
     */
    SchemaVersion toVersion();

    /**
     * Apply the migration to the given JSON tree in place.
     *
     * @param root The mutable JSON object to transform
     * @throws IllegalStateException if the migration cannot be applied
     */
    void migrate(ObjectNode root);

    /**
     * Forward migration: upgrades from an older version to a newer version.
     * Typically adds new fields with default values.
     */
    non-sealed interface Forward extends SchemaMigrator {
    }

    /**
     * Rollback migration: downgrades from a newer version to an older version.
     * Typically removes fields that do not exist in the target version.
     */
    non-sealed interface Rollback extends SchemaMigrator {
    }
}
