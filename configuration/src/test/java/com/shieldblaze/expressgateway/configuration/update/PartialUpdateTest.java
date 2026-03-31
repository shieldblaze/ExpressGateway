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
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PartialUpdateTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    // -----------------------------------------------------------------------
    // JSON Merge Patch (RFC 7396)
    // -----------------------------------------------------------------------

    @Test
    void mergePatchUpdatesField() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "old");
        target.put("port", 80);

        ObjectNode patch = MAPPER.createObjectNode();
        patch.put("name", "new");

        ObjectNode result = PartialUpdate.applyMergePatch(target, patch, false);
        assertEquals("new", result.get("name").asText());
        assertEquals(80, result.get("port").asInt()); // Untouched
    }

    @Test
    void mergePatchRemovesFieldWithNull() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "test");
        target.put("description", "to-remove");

        ObjectNode patch = MAPPER.createObjectNode();
        patch.putNull("description");

        ObjectNode result = PartialUpdate.applyMergePatch(target, patch, false);
        assertEquals("test", result.get("name").asText());
        assertFalse(result.has("description"));
    }

    @Test
    void mergePatchAddsNewField() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "test");

        ObjectNode patch = MAPPER.createObjectNode();
        patch.put("newField", "value");

        ObjectNode result = PartialUpdate.applyMergePatch(target, patch, false);
        assertEquals("value", result.get("newField").asText());
    }

    @Test
    void mergePatchNestedObjectMerge() {
        ObjectNode target = MAPPER.createObjectNode();
        ObjectNode nested = MAPPER.createObjectNode();
        nested.put("a", 1);
        nested.put("b", 2);
        target.set("nested", nested);

        ObjectNode patch = MAPPER.createObjectNode();
        ObjectNode patchNested = MAPPER.createObjectNode();
        patchNested.put("a", 10); // Update
        patch.set("nested", patchNested);

        ObjectNode result = PartialUpdate.applyMergePatch(target, patch, false);
        assertEquals(10, result.get("nested").get("a").asInt());
        assertEquals(2, result.get("nested").get("b").asInt()); // Untouched
    }

    @Test
    void mergePatchDryRunDoesNotModifyTarget() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "original");

        ObjectNode patch = MAPPER.createObjectNode();
        patch.put("name", "modified");

        ObjectNode result = PartialUpdate.applyMergePatch(target, patch, true);
        assertEquals("modified", result.get("name").asText());
        assertEquals("original", target.get("name").asText()); // Original unchanged
    }

    // -----------------------------------------------------------------------
    // JSON Patch (RFC 6902)
    // -----------------------------------------------------------------------

    @Test
    void jsonPatchAddOperation() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "test");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("add", "/port", MAPPER.valueToTree(8080), null)
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertEquals(8080, result.get("port").asInt());
    }

    @Test
    void jsonPatchRemoveOperation() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "test");
        target.put("toRemove", "value");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("remove", "/toRemove", null, null)
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertFalse(result.has("toRemove"));
        assertTrue(result.has("name"));
    }

    @Test
    void jsonPatchReplaceOperation() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("port", 80);

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("replace", "/port", MAPPER.valueToTree(443), null)
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertEquals(443, result.get("port").asInt());
    }

    @Test
    void jsonPatchReplaceNonExistentPathThrows() {
        ObjectNode target = MAPPER.createObjectNode();

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("replace", "/missing", MAPPER.valueToTree("value"), null)
        );

        assertThrows(PartialUpdate.PatchException.class,
                () -> PartialUpdate.applyJsonPatch(target, ops, false));
    }

    @Test
    void jsonPatchMoveOperation() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("source", "value");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("move", "/destination", null, "/source")
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertFalse(result.has("source"));
        assertEquals("value", result.get("destination").asText());
    }

    @Test
    void jsonPatchCopyOperation() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("source", "value");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("copy", "/copy", null, "/source")
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertEquals("value", result.get("source").asText());
        assertEquals("value", result.get("copy").asText());
    }

    @Test
    void jsonPatchTestOperationPasses() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "test");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("test", "/name", MAPPER.valueToTree("test"), null)
        );

        // Should not throw
        PartialUpdate.applyJsonPatch(target, ops, false);
    }

    @Test
    void jsonPatchTestOperationFails() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "actual");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("test", "/name", MAPPER.valueToTree("expected"), null)
        );

        assertThrows(PartialUpdate.PatchException.class,
                () -> PartialUpdate.applyJsonPatch(target, ops, false));
    }

    @Test
    void jsonPatchDryRunDoesNotModifyTarget() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "original");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("replace", "/name", MAPPER.valueToTree("modified"), null)
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, true);
        assertEquals("modified", result.get("name").asText());
        assertEquals("original", target.get("name").asText()); // Original unchanged
    }

    @Test
    void jsonPatchAtomicOnFailure() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("field1", "original1");
        target.put("field2", "original2");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("replace", "/field1", MAPPER.valueToTree("modified1"), null),
                new PartialUpdate.PatchOperation("replace", "/nonExistent", MAPPER.valueToTree("fail"), null)
        );

        assertThrows(PartialUpdate.PatchException.class,
                () -> PartialUpdate.applyJsonPatch(target, ops, false));
        // Original should be unchanged because of atomic copy-on-write
        assertEquals("original1", target.get("field1").asText());
        assertEquals("original2", target.get("field2").asText());
    }

    @Test
    void jsonPatchMultipleOperations() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "old");
        target.put("port", 80);
        target.put("toRemove", "bye");

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("replace", "/name", MAPPER.valueToTree("new"), null),
                new PartialUpdate.PatchOperation("replace", "/port", MAPPER.valueToTree(443), null),
                new PartialUpdate.PatchOperation("remove", "/toRemove", null, null),
                new PartialUpdate.PatchOperation("add", "/added", MAPPER.valueToTree("hello"), null)
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertEquals("new", result.get("name").asText());
        assertEquals(443, result.get("port").asInt());
        assertFalse(result.has("toRemove"));
        assertEquals("hello", result.get("added").asText());
    }

    @Test
    void jsonPatchNestedOperations() {
        ObjectNode target = MAPPER.createObjectNode();
        ObjectNode nested = MAPPER.createObjectNode();
        nested.put("a", 1);
        nested.put("b", 2);
        target.set("nested", nested);

        List<PartialUpdate.PatchOperation> ops = List.of(
                new PartialUpdate.PatchOperation("replace", "/nested/a", MAPPER.valueToTree(10), null),
                new PartialUpdate.PatchOperation("add", "/nested/c", MAPPER.valueToTree(3), null)
        );

        ObjectNode result = PartialUpdate.applyJsonPatch(target, ops, false);
        assertEquals(10, result.get("nested").get("a").asInt());
        assertEquals(2, result.get("nested").get("b").asInt());
        assertEquals(3, result.get("nested").get("c").asInt());
    }

    @Test
    void emptyPatchListReturnsTarget() {
        ObjectNode target = MAPPER.createObjectNode();
        target.put("name", "test");

        ObjectNode result = PartialUpdate.applyJsonPatch(target, List.of(), false);
        assertEquals("test", result.get("name").asText());
    }

    @Test
    void invalidPatchOperationRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new PartialUpdate.PatchOperation("invalid", "/path", null, null));
    }

    @Test
    void nullPatchOperationOpRejected() {
        assertThrows(NullPointerException.class,
                () -> new PartialUpdate.PatchOperation(null, "/path", null, null));
    }

    @Test
    void nullPatchOperationPathRejected() {
        assertThrows(NullPointerException.class,
                () -> new PartialUpdate.PatchOperation("add", null, null, null));
    }
}
