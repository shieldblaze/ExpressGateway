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
package com.shieldblaze.expressgateway.configuration.update;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Supports partial configuration updates via JSON Merge Patch (RFC 7396)
 * and JSON Patch (RFC 6902).
 *
 * <p>Both modes support a dry-run option that validates the result without
 * modifying the original document.</p>
 *
 * <p>All operations are atomic: if validation fails during dry-run, the
 * original document is never modified.</p>
 */
public final class PartialUpdate {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    private PartialUpdate() {
        // Static utility class
    }

    // -----------------------------------------------------------------------
    // JSON Merge Patch (RFC 7396)
    // -----------------------------------------------------------------------

    /**
     * Apply a JSON Merge Patch (RFC 7396) to a target JSON object.
     *
     * <p>Merge semantics per RFC 7396:</p>
     * <ul>
     *   <li>If the patch value is an object, recursively merge</li>
     *   <li>If the patch value is null, remove the field</li>
     *   <li>Otherwise, replace the field value</li>
     * </ul>
     *
     * @param target The target JSON object to patch
     * @param patch  The merge patch to apply
     * @param dryRun If true, returns the result without modifying target
     * @return The patched JSON object
     */
    public static ObjectNode applyMergePatch(ObjectNode target, JsonNode patch, boolean dryRun) {
        Objects.requireNonNull(target, "target");
        Objects.requireNonNull(patch, "patch");

        ObjectNode workingCopy = dryRun ? target.deepCopy() : target;
        return mergePatchInternal(workingCopy, patch);
    }

    private static ObjectNode mergePatchInternal(ObjectNode target, JsonNode patch) {
        if (!patch.isObject()) {
            // Per RFC 7396: if patch is not an object, replace the whole thing
            // This case should not happen at the top level (caller sends ObjectNode)
            // but handles nested cases.
            return target;
        }

        for (Map.Entry<String, JsonNode> entry : patch.properties()) {
            String key = entry.getKey();
            JsonNode value = entry.getValue();

            if (value.isNull()) {
                // RFC 7396: null means remove the field
                target.remove(key);
            } else if (value.isObject() && target.has(key) && target.get(key).isObject()) {
                // Recursively merge objects
                mergePatchInternal((ObjectNode) target.get(key), value);
            } else {
                // Replace or add the field
                target.set(key, value.deepCopy());
            }
        }

        return target;
    }

    // -----------------------------------------------------------------------
    // JSON Patch (RFC 6902)
    // -----------------------------------------------------------------------

    /**
     * A single JSON Patch operation (RFC 6902).
     *
     * @param op    The operation ("add", "remove", "replace", "move", "copy", "test")
     * @param path  The JSON Pointer (RFC 6901) targeting the location
     * @param value The value for add/replace/test operations (null for remove/move/copy)
     * @param from  The source JSON Pointer for move/copy operations (null otherwise)
     */
    public record PatchOperation(String op, String path, JsonNode value, String from) {

        private static final java.util.Set<String> VALID_OPS =
                java.util.Set.of("add", "remove", "replace", "move", "copy", "test");

        public PatchOperation {
            Objects.requireNonNull(op, "op");
            if (!VALID_OPS.contains(op)) {
                throw new IllegalArgumentException("op must be one of " + VALID_OPS + ", got: " + op);
            }
            Objects.requireNonNull(path, "path");
        }
    }

    /**
     * Apply a list of JSON Patch operations (RFC 6902) to a target JSON object.
     *
     * <p>Operations are applied sequentially. If any operation fails, a
     * {@link PatchException} is thrown and no changes are persisted
     * (operations are applied to a deep copy that is assigned back only on success).</p>
     *
     * @param target     The target JSON object to patch
     * @param operations The list of patch operations
     * @param dryRun     If true, returns the result without modifying target
     * @return The patched JSON object
     * @throws PatchException if any operation fails
     */
    public static ObjectNode applyJsonPatch(ObjectNode target, List<PatchOperation> operations, boolean dryRun) {
        Objects.requireNonNull(target, "target");
        Objects.requireNonNull(operations, "operations");
        if (operations.isEmpty()) {
            return dryRun ? target.deepCopy() : target;
        }

        // Always work on a copy for atomicity; assign back to target only if not dry-run
        ObjectNode workingCopy = target.deepCopy();

        for (int i = 0; i < operations.size(); i++) {
            PatchOperation op = operations.get(i);
            try {
                applyOperation(workingCopy, op);
            } catch (Exception e) {
                throw new PatchException("Operation " + i + " (" + op.op() + " " + op.path() + ") failed: " + e.getMessage(), e);
            }
        }

        if (!dryRun) {
            // Copy results back to original target
            Iterator<String> fieldNames = target.fieldNames();
            while (fieldNames.hasNext()) {
                fieldNames.next();
                fieldNames.remove();
            }
            target.setAll(workingCopy);
        }

        return workingCopy;
    }

    private static void applyOperation(ObjectNode root, PatchOperation op) {
        switch (op.op()) {
            case "add" -> applyAdd(root, op.path(), op.value());
            case "remove" -> applyRemove(root, op.path());
            case "replace" -> applyReplace(root, op.path(), op.value());
            case "move" -> applyMove(root, op.path(), op.from());
            case "copy" -> applyCopy(root, op.path(), op.from());
            case "test" -> applyTest(root, op.path(), op.value());
            default -> throw new IllegalArgumentException("Unknown operation: " + op.op());
        }
    }

    private static void applyAdd(ObjectNode root, String path, JsonNode value) {
        if (value == null) {
            throw new IllegalArgumentException("add operation requires a value");
        }
        String[] segments = parsePath(path);
        ObjectNode parent = navigateToParent(root, segments);
        String lastSegment = segments[segments.length - 1];

        if (lastSegment.equals("-") && parent.isArray()) {
            throw new IllegalArgumentException("Array append not supported on ObjectNode");
        }
        parent.set(lastSegment, value.deepCopy());
    }

    private static void applyRemove(ObjectNode root, String path) {
        String[] segments = parsePath(path);
        ObjectNode parent = navigateToParent(root, segments);
        String lastSegment = segments[segments.length - 1];

        if (!parent.has(lastSegment)) {
            throw new IllegalArgumentException("Path does not exist: " + path);
        }
        parent.remove(lastSegment);
    }

    private static void applyReplace(ObjectNode root, String path, JsonNode value) {
        if (value == null) {
            throw new IllegalArgumentException("replace operation requires a value");
        }
        String[] segments = parsePath(path);
        ObjectNode parent = navigateToParent(root, segments);
        String lastSegment = segments[segments.length - 1];

        if (!parent.has(lastSegment)) {
            throw new IllegalArgumentException("Path does not exist for replace: " + path);
        }
        parent.set(lastSegment, value.deepCopy());
    }

    private static void applyMove(ObjectNode root, String toPath, String fromPath) {
        if (fromPath == null) {
            throw new IllegalArgumentException("move operation requires a 'from' path");
        }
        String[] fromSegments = parsePath(fromPath);
        ObjectNode fromParent = navigateToParent(root, fromSegments);
        String fromLast = fromSegments[fromSegments.length - 1];

        if (!fromParent.has(fromLast)) {
            throw new IllegalArgumentException("Source path does not exist: " + fromPath);
        }
        JsonNode moved = fromParent.remove(fromLast);
        applyAdd(root, toPath, moved);
    }

    private static void applyCopy(ObjectNode root, String toPath, String fromPath) {
        if (fromPath == null) {
            throw new IllegalArgumentException("copy operation requires a 'from' path");
        }
        String[] fromSegments = parsePath(fromPath);
        ObjectNode fromParent = navigateToParent(root, fromSegments);
        String fromLast = fromSegments[fromSegments.length - 1];

        if (!fromParent.has(fromLast)) {
            throw new IllegalArgumentException("Source path does not exist: " + fromPath);
        }
        JsonNode copied = fromParent.get(fromLast);
        applyAdd(root, toPath, copied.deepCopy());
    }

    private static void applyTest(ObjectNode root, String path, JsonNode expected) {
        if (expected == null) {
            throw new IllegalArgumentException("test operation requires a value");
        }
        String[] segments = parsePath(path);
        ObjectNode parent = navigateToParent(root, segments);
        String lastSegment = segments[segments.length - 1];

        if (!parent.has(lastSegment)) {
            throw new IllegalArgumentException("Path does not exist for test: " + path);
        }
        JsonNode actual = parent.get(lastSegment);
        if (!actual.equals(expected)) {
            throw new IllegalArgumentException(
                    "Test failed at " + path + ": expected " + expected + " but got " + actual);
        }
    }

    /**
     * Parse a JSON Pointer (RFC 6901) into path segments.
     * E.g., "/foo/bar" -> ["foo", "bar"]
     */
    private static String[] parsePath(String path) {
        if (path == null || path.isEmpty() || path.equals("/")) {
            throw new IllegalArgumentException("Invalid JSON Pointer path: " + path);
        }
        if (!path.startsWith("/")) {
            throw new IllegalArgumentException("JSON Pointer must start with '/', got: " + path);
        }
        String[] parts = path.substring(1).split("/");
        // Unescape RFC 6901 escapes: ~1 -> /, ~0 -> ~
        for (int i = 0; i < parts.length; i++) {
            parts[i] = parts[i].replace("~1", "/").replace("~0", "~");
        }
        return parts;
    }

    /**
     * Navigate to the parent ObjectNode of the target path segment.
     */
    private static ObjectNode navigateToParent(ObjectNode root, String[] segments) {
        ObjectNode current = root;
        for (int i = 0; i < segments.length - 1; i++) {
            JsonNode child = current.get(segments[i]);
            if (child == null || !child.isObject()) {
                throw new IllegalArgumentException("Path segment '" + segments[i] + "' does not exist or is not an object");
            }
            current = (ObjectNode) child;
        }
        return current;
    }

    /**
     * Exception thrown when a JSON Patch operation fails.
     */
    public static final class PatchException extends RuntimeException {
        public PatchException(String message, Throwable cause) {
            super(message, cause);
        }
    }
}
