package com.shieldblaze.expressgateway.controlplane.agent;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Path;
import java.time.Instant;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class LKGStoreTest {

    @TempDir
    Path tempDir;

    private static ConfigResource createResource(String name) {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", name);
        return new ConfigResource(
                id,
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                null,
                new ClusterSpec(name, "round-robin", "default-hc", 100, 30)
        );
    }

    @Test
    void saveAndLoadRevision() throws IOException {
        Path file = tempDir.resolve("lkg.json");
        LKGStore store = new LKGStore(file);

        store.save(List.of(createResource("prod-1")), 42);

        assertEquals(42, store.loadRevision());
    }

    @Test
    void loadRevisionReturnsNegativeOneWhenNoFile() {
        Path file = tempDir.resolve("nonexistent.json");
        LKGStore store = new LKGStore(file);

        assertEquals(-1, store.loadRevision());
    }

    @Test
    void existsReturnsFalseInitiallyTrueAfterSave() throws IOException {
        Path file = tempDir.resolve("lkg.json");
        LKGStore store = new LKGStore(file);

        assertFalse(store.exists(), "file should not exist before save");

        store.save(List.of(createResource("prod-1")), 1);

        assertTrue(store.exists(), "file should exist after save");
    }

    @Test
    void saveCreatesParentDirectories() throws IOException {
        Path file = tempDir.resolve("nested/dir/lkg.json");
        LKGStore store = new LKGStore(file);

        assertFalse(store.exists(), "file should not exist before save");

        store.save(List.of(createResource("prod-1")), 10);

        assertTrue(store.exists(), "file should exist after save in nested directory");
        assertEquals(10, store.loadRevision());
    }

    @Test
    void saveOverwritesPreviousRevision() throws IOException {
        Path file = tempDir.resolve("lkg.json");
        LKGStore store = new LKGStore(file);

        store.save(List.of(createResource("v1")), 1);
        assertEquals(1, store.loadRevision());

        store.save(List.of(createResource("v2")), 99);
        assertEquals(99, store.loadRevision());
    }

    @Test
    void nullStoragePathThrows() {
        assertThrows(NullPointerException.class, () -> new LKGStore(null));
    }

    @Test
    void saveMultipleResourcesAndLoadRevision() throws IOException {
        Path file = tempDir.resolve("lkg.json");
        LKGStore store = new LKGStore(file);

        List<ConfigResource> resources = List.of(
                createResource("svc-a"),
                createResource("svc-b"),
                createResource("svc-c")
        );

        store.save(resources, 7);

        assertEquals(7, store.loadRevision());
        assertTrue(store.exists());
    }

    @Test
    void saveEmptyResourceListAndLoadRevision() throws IOException {
        Path file = tempDir.resolve("lkg.json");
        LKGStore store = new LKGStore(file);

        store.save(List.of(), 0);

        // Revision 0 is not technically valid (version >= 1 in ConfigResource), but the store
        // only cares about the revision field, which is a plain long.
        // Actually revision is store metadata, not ConfigResource.version. 0 is fine.
        assertEquals(0, store.loadRevision());
        assertTrue(store.exists());
    }
}
