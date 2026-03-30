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

import com.fasterxml.jackson.databind.node.ObjectNode;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Queue;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * Manages schema versioning for configuration schemas.
 *
 * <p>Supports:</p>
 * <ul>
 *   <li>Registration of forward and rollback migrations</li>
 *   <li>Chained migration (e.g., v1.0.0 -> v1.1.0 -> v1.2.0)</li>
 *   <li>Backward compatibility checking</li>
 *   <li>Version tag embedding in JSON documents</li>
 * </ul>
 *
 * <p>Thread-safe: migrations are stored in a {@link CopyOnWriteArrayList} and
 * all mutation methods are idempotent.</p>
 */
public final class ConfigVersioning {

    private static final Logger logger = LogManager.getLogger(ConfigVersioning.class);
    private static final String VERSION_FIELD = "_schemaVersion";

    private final SchemaVersion currentVersion;
    private final CopyOnWriteArrayList<SchemaMigrator> migrations = new CopyOnWriteArrayList<>();

    /**
     * Create a new ConfigVersioning instance for the given current schema version.
     *
     * @param currentVersion The current (latest) schema version
     */
    public ConfigVersioning(SchemaVersion currentVersion) {
        this.currentVersion = Objects.requireNonNull(currentVersion, "currentVersion");
    }

    /**
     * Register a migration step.
     *
     * @param migrator The migration to register
     * @throws IllegalArgumentException if a migration with the same from/to versions already exists
     */
    public void registerMigration(SchemaMigrator migrator) {
        Objects.requireNonNull(migrator, "migrator");
        Objects.requireNonNull(migrator.fromVersion(), "migrator.fromVersion()");
        Objects.requireNonNull(migrator.toVersion(), "migrator.toVersion()");

        for (SchemaMigrator existing : migrations) {
            if (existing.fromVersion().equals(migrator.fromVersion())
                    && existing.toVersion().equals(migrator.toVersion())
                    && existing.getClass().isAssignableFrom(migrator.getClass())) {
                throw new IllegalArgumentException(
                        "Migration from " + migrator.fromVersion() + " to " + migrator.toVersion() + " already registered");
            }
        }
        migrations.add(migrator);
        logger.info("Registered migration: {} -> {} ({})",
                migrator.fromVersion(), migrator.toVersion(),
                migrator instanceof SchemaMigrator.Forward ? "forward" : "rollback");
    }

    /**
     * Migrate a JSON configuration document from its embedded version to the current version.
     *
     * <p>If the document has no version tag, it is assumed to be at version 1.0.0.</p>
     *
     * <p>Migration is applied to a deep copy of the document. The original is only
     * replaced with the migrated version if all migration steps succeed.</p>
     *
     * @param root The mutable JSON object to migrate
     * @return The migrated JSON object (same instance if already current, or mutated on success)
     * @throws IllegalStateException if no migration path exists
     */
    public ObjectNode migrateToCurrentVersion(ObjectNode root) {
        Objects.requireNonNull(root, "root");
        SchemaVersion docVersion = extractVersion(root);

        if (docVersion.equals(currentVersion)) {
            return root;
        }

        List<SchemaMigrator> path = findMigrationPath(docVersion, currentVersion);
        if (path.isEmpty()) {
            throw new IllegalStateException(
                    "No migration path from " + docVersion + " to " + currentVersion);
        }

        // Work on a deep copy so original is untouched if a step fails
        ObjectNode copy = root.deepCopy();

        for (SchemaMigrator step : path) {
            logger.debug("Applying migration: {} -> {}", step.fromVersion(), step.toVersion());
            step.migrate(copy);
        }

        stampVersion(copy, currentVersion);

        // Success: apply all changes to the original
        root.removeAll();
        root.setAll(copy);
        return root;
    }

    /**
     * Rollback a JSON configuration document to a target version.
     *
     * @param root          The mutable JSON object to rollback
     * @param targetVersion The target version to rollback to
     * @return The rolled-back JSON object (same instance, mutated in place)
     * @throws IllegalStateException if no rollback path exists
     */
    public ObjectNode rollbackToVersion(ObjectNode root, SchemaVersion targetVersion) {
        Objects.requireNonNull(root, "root");
        Objects.requireNonNull(targetVersion, "targetVersion");

        SchemaVersion docVersion = extractVersion(root);
        if (docVersion.equals(targetVersion)) {
            return root;
        }

        List<SchemaMigrator> path = findRollbackPath(docVersion, targetVersion);
        if (path.isEmpty()) {
            throw new IllegalStateException(
                    "No rollback path from " + docVersion + " to " + targetVersion);
        }

        for (SchemaMigrator step : path) {
            logger.debug("Applying rollback: {} -> {}", step.fromVersion(), step.toVersion());
            step.migrate(root);
        }

        stampVersion(root, targetVersion);
        return root;
    }

    /**
     * Check if a document at the given version is compatible with the current version.
     *
     * @param docVersion The document's version
     * @return {@code true} if the document can be read by the current version
     */
    public boolean isCompatible(SchemaVersion docVersion) {
        Objects.requireNonNull(docVersion, "docVersion");
        return currentVersion.isCompatibleWith(docVersion);
    }

    /**
     * Stamp a version tag into the JSON document.
     *
     * @param root    The JSON object to stamp
     * @param version The version to embed
     */
    public static void stampVersion(ObjectNode root, SchemaVersion version) {
        root.put(VERSION_FIELD, version.toString());
    }

    /**
     * Extract the schema version from a JSON document.
     * Returns {@link SchemaVersion#V1_0_0} if no version tag is present.
     *
     * @param root The JSON object to read
     * @return The extracted version
     */
    public static SchemaVersion extractVersion(ObjectNode root) {
        if (root.has(VERSION_FIELD) && !root.get(VERSION_FIELD).isNull()) {
            return SchemaVersion.parse(root.get(VERSION_FIELD).asText());
        }
        return SchemaVersion.V1_0_0;
    }

    /**
     * Returns the current schema version.
     */
    public SchemaVersion currentVersion() {
        return currentVersion;
    }

    /**
     * Returns an unmodifiable view of all registered migrations.
     */
    public List<SchemaMigrator> registeredMigrations() {
        return Collections.unmodifiableList(new ArrayList<>(migrations));
    }

    /**
     * Find a forward migration path from source to target using registered Forward migrators.
     */
    private List<SchemaMigrator> findMigrationPath(SchemaVersion from, SchemaVersion to) {
        return findPath(from, to, SchemaMigrator.Forward.class);
    }

    /**
     * Find a rollback path from source to target using registered Rollback migrators.
     */
    private List<SchemaMigrator> findRollbackPath(SchemaVersion from, SchemaVersion to) {
        return findPath(from, to, SchemaMigrator.Rollback.class);
    }

    /**
     * BFS path-finding through registered migrations of the given type.
     * Builds an adjacency map from the registered migrations, then performs
     * breadth-first search from 'from' to 'to' to find the shortest path.
     */
    private List<SchemaMigrator> findPath(SchemaVersion from, SchemaVersion to, Class<? extends SchemaMigrator> type) {
        // Build adjacency map: version -> list of migrations starting from that version
        Map<SchemaVersion, List<SchemaMigrator>> adjacency = new HashMap<>();
        for (SchemaMigrator m : migrations) {
            if (type.isInstance(m)) {
                adjacency.computeIfAbsent(m.fromVersion(), k -> new ArrayList<>()).add(m);
            }
        }

        // BFS
        Queue<SchemaVersion> queue = new LinkedList<>();
        Map<SchemaVersion, SchemaMigrator> cameFrom = new HashMap<>();
        queue.add(from);
        cameFrom.put(from, null); // sentinel: 'from' has no predecessor

        while (!queue.isEmpty()) {
            SchemaVersion current = queue.poll();
            if (current.equals(to)) {
                // Reconstruct path
                List<SchemaMigrator> path = new ArrayList<>();
                SchemaVersion step = to;
                while (!step.equals(from)) {
                    SchemaMigrator migrator = cameFrom.get(step);
                    path.add(migrator);
                    step = migrator.fromVersion();
                }
                Collections.reverse(path);
                return Collections.unmodifiableList(path);
            }

            List<SchemaMigrator> neighbors = adjacency.getOrDefault(current, List.of());
            for (SchemaMigrator m : neighbors) {
                if (!cameFrom.containsKey(m.toVersion())) {
                    cameFrom.put(m.toVersion(), m);
                    queue.add(m.toVersion());
                }
            }
        }

        return List.of(); // No path found
    }
}
