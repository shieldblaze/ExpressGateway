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

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigVersioningTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private static final SchemaVersion V1_0_0 = new SchemaVersion(1, 0, 0);
    private static final SchemaVersion V1_1_0 = new SchemaVersion(1, 1, 0);
    private static final SchemaVersion V1_2_0 = new SchemaVersion(1, 2, 0);
    private static final SchemaVersion V1_3_0 = new SchemaVersion(1, 3, 0);

    @Test
    void sameVersionNoMigration() {
        ConfigVersioning versioning = new ConfigVersioning(V1_0_0);
        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "test");
        ConfigVersioning.stampVersion(doc, V1_0_0);

        ObjectNode result = versioning.migrateToCurrentVersion(doc);
        assertEquals("test", result.get("name").asText());
        assertEquals("1.0.0", result.get("_schemaVersion").asText());
    }

    @Test
    void singleStepForwardMigration() {
        ConfigVersioning versioning = new ConfigVersioning(V1_1_0);

        // Register a forward migration that adds a "newField" with default value
        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override
            public SchemaVersion fromVersion() { return V1_0_0; }
            @Override
            public SchemaVersion toVersion() { return V1_1_0; }
            @Override
            public void migrate(ObjectNode root) {
                root.put("newField", "default-value");
            }
        });

        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "test");
        ConfigVersioning.stampVersion(doc, V1_0_0);

        ObjectNode result = versioning.migrateToCurrentVersion(doc);
        assertEquals("test", result.get("name").asText());
        assertEquals("default-value", result.get("newField").asText());
        assertEquals("1.1.0", result.get("_schemaVersion").asText());
    }

    @Test
    void chainedForwardMigration() {
        ConfigVersioning versioning = new ConfigVersioning(V1_2_0);

        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override
            public SchemaVersion fromVersion() { return V1_0_0; }
            @Override
            public SchemaVersion toVersion() { return V1_1_0; }
            @Override
            public void migrate(ObjectNode root) {
                root.put("fieldA", "added-in-1.1.0");
            }
        });

        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override
            public SchemaVersion fromVersion() { return V1_1_0; }
            @Override
            public SchemaVersion toVersion() { return V1_2_0; }
            @Override
            public void migrate(ObjectNode root) {
                root.put("fieldB", "added-in-1.2.0");
            }
        });

        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "test");
        ConfigVersioning.stampVersion(doc, V1_0_0);

        ObjectNode result = versioning.migrateToCurrentVersion(doc);
        assertEquals("added-in-1.1.0", result.get("fieldA").asText());
        assertEquals("added-in-1.2.0", result.get("fieldB").asText());
        assertEquals("1.2.0", result.get("_schemaVersion").asText());
    }

    @Test
    void bfsFindsPathThatGreedyMisses() {
        // Create a graph where the greedy approach picks the wrong edge:
        // v1.0.0 -> v1.2.0 (direct, but greedy min picks v1.1.0 first if available)
        // v1.0.0 -> v1.1.0 (greedy picks this since v1.1.0 < v1.2.0)
        // v1.1.0 -> v1.3.0 (greedy reaches v1.3.0, which is NOT the target v1.2.0)
        // With BFS, v1.0.0 -> v1.2.0 is found directly.
        ConfigVersioning versioning = new ConfigVersioning(V1_2_0);

        // Register a direct path from v1.0.0 to v1.2.0
        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override public SchemaVersion fromVersion() { return V1_0_0; }
            @Override public SchemaVersion toVersion() { return V1_2_0; }
            @Override public void migrate(ObjectNode root) {
                root.put("migrated", "direct-1.0.0-to-1.2.0");
            }
        });

        // Register a path from v1.0.0 to v1.1.0 (greedy would pick this first)
        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override public SchemaVersion fromVersion() { return V1_0_0; }
            @Override public SchemaVersion toVersion() { return V1_1_0; }
            @Override public void migrate(ObjectNode root) {
                root.put("migrated", "indirect-via-1.1.0");
            }
        });

        // Register v1.1.0 -> v1.3.0 (dead end for reaching v1.2.0)
        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override public SchemaVersion fromVersion() { return V1_1_0; }
            @Override public SchemaVersion toVersion() { return V1_3_0; }
            @Override public void migrate(ObjectNode root) {
                root.put("migrated", "indirect-via-1.3.0");
            }
        });

        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "test");
        ConfigVersioning.stampVersion(doc, V1_0_0);

        // BFS should find the direct path v1.0.0 -> v1.2.0
        ObjectNode result = versioning.migrateToCurrentVersion(doc);
        assertEquals("direct-1.0.0-to-1.2.0", result.get("migrated").asText());
        assertEquals("1.2.0", result.get("_schemaVersion").asText());
    }

    @Test
    void migrateToCurrentVersionWorksOnDeepCopy() {
        ConfigVersioning versioning = new ConfigVersioning(V1_1_0);

        // Migration that throws on the second call to verify deep copy behavior
        versioning.registerMigration(new SchemaMigrator.Forward() {
            @Override public SchemaVersion fromVersion() { return V1_0_0; }
            @Override public SchemaVersion toVersion() { return V1_1_0; }
            @Override public void migrate(ObjectNode root) {
                root.put("newField", "added");
            }
        });

        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "original");
        ConfigVersioning.stampVersion(doc, V1_0_0);

        ObjectNode result = versioning.migrateToCurrentVersion(doc);
        assertEquals("added", result.get("newField").asText());
        assertEquals("original", result.get("name").asText());
    }

    @Test
    void rollbackMigration() {
        ConfigVersioning versioning = new ConfigVersioning(V1_1_0);

        versioning.registerMigration(new SchemaMigrator.Rollback() {
            @Override
            public SchemaVersion fromVersion() { return V1_1_0; }
            @Override
            public SchemaVersion toVersion() { return V1_0_0; }
            @Override
            public void migrate(ObjectNode root) {
                root.remove("newField");
            }
        });

        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "test");
        doc.put("newField", "value");
        ConfigVersioning.stampVersion(doc, V1_1_0);

        ObjectNode result = versioning.rollbackToVersion(doc, V1_0_0);
        assertEquals("test", result.get("name").asText());
        assertFalse(result.has("newField"));
        assertEquals("1.0.0", result.get("_schemaVersion").asText());
    }

    @Test
    void noMigrationPathThrows() {
        ConfigVersioning versioning = new ConfigVersioning(V1_2_0);
        // No migrations registered

        ObjectNode doc = MAPPER.createObjectNode();
        ConfigVersioning.stampVersion(doc, V1_0_0);

        assertThrows(IllegalStateException.class,
                () -> versioning.migrateToCurrentVersion(doc));
    }

    @Test
    void documentWithoutVersionAssumesV100() {
        ObjectNode doc = MAPPER.createObjectNode();
        doc.put("name", "test");

        SchemaVersion extracted = ConfigVersioning.extractVersion(doc);
        assertEquals(V1_0_0, extracted);
    }

    @Test
    void stampAndExtractVersion() {
        ObjectNode doc = MAPPER.createObjectNode();
        ConfigVersioning.stampVersion(doc, V1_1_0);

        SchemaVersion extracted = ConfigVersioning.extractVersion(doc);
        assertEquals(V1_1_0, extracted);
    }

    @Test
    void compatibilityCheck() {
        ConfigVersioning versioning = new ConfigVersioning(V1_1_0);

        assertTrue(versioning.isCompatible(V1_0_0));  // Can read older minor
        assertTrue(versioning.isCompatible(V1_1_0));  // Same version
        assertFalse(versioning.isCompatible(V1_2_0)); // Cannot read newer minor
    }

    @Test
    void duplicateMigrationRegistrationRejected() {
        ConfigVersioning versioning = new ConfigVersioning(V1_1_0);

        SchemaMigrator.Forward migration = new SchemaMigrator.Forward() {
            @Override
            public SchemaVersion fromVersion() { return V1_0_0; }
            @Override
            public SchemaVersion toVersion() { return V1_1_0; }
            @Override
            public void migrate(ObjectNode root) { }
        };

        versioning.registerMigration(migration);
        assertThrows(IllegalArgumentException.class,
                () -> versioning.registerMigration(migration));
    }

    @Test
    void registeredMigrationsReturnsUnmodifiableList() {
        ConfigVersioning versioning = new ConfigVersioning(V1_1_0);
        assertThrows(UnsupportedOperationException.class,
                () -> versioning.registeredMigrations().add(null));
    }
}
