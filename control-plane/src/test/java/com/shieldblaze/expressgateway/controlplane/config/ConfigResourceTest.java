package com.shieldblaze.expressgateway.controlplane.config;

import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigResourceTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private final ConfigResourceId sampleId =
            new ConfigResourceId("cluster", "global", "prod");
    private final Instant now = Instant.now();

    private ConfigResource sampleResource(Map<String, String> labels) {
        return new ConfigResource(
                sampleId,
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1,
                now,
                now,
                "admin",
                labels,
                new TestSpec("test")
        );
    }

    @Test
    void validConstructionWithAllFields() {
        Map<String, String> labels = Map.of("env", "production");
        ConfigResource resource = sampleResource(labels);

        assertEquals(sampleId, resource.id());
        assertEquals(ConfigKind.CLUSTER, resource.kind());
        assertEquals(1, resource.version());
        assertEquals(now, resource.createdAt());
        assertEquals(now, resource.updatedAt());
        assertEquals("admin", resource.createdBy());
        assertEquals(Map.of("env", "production"), resource.labels());
    }

    @Test
    void versionBelowOneThrows() {
        assertThrows(IllegalArgumentException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                0, now, now, "admin", Map.of(), new TestSpec("test")));

        assertThrows(IllegalArgumentException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                -1, now, now, "admin", Map.of(), new TestSpec("test")));
    }

    @Test
    void nullIdThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                null, ConfigKind.CLUSTER, new ConfigScope.Global(),
                1, now, now, "admin", Map.of(), new TestSpec("test")));
    }

    @Test
    void nullKindThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                sampleId, null, new ConfigScope.Global(),
                1, now, now, "admin", Map.of(), new TestSpec("test")));
    }

    @Test
    void nullScopeThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, null,
                1, now, now, "admin", Map.of(), new TestSpec("test")));
    }

    @Test
    void nullCreatedAtThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                1, null, now, "admin", Map.of(), new TestSpec("test")));
    }

    @Test
    void nullUpdatedAtThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                1, now, null, "admin", Map.of(), new TestSpec("test")));
    }

    @Test
    void nullCreatedByThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                1, now, now, null, Map.of(), new TestSpec("test")));
    }

    @Test
    void nullSpecThrows() {
        assertThrows(NullPointerException.class, () -> new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                1, now, now, "admin", Map.of(), null));
    }

    @Test
    void labelsAreDefensivelyCopied() {
        HashMap<String, String> mutableLabels = new HashMap<>();
        mutableLabels.put("env", "prod");

        ConfigResource resource = sampleResource(mutableLabels);

        // Mutating the original map should not affect the resource
        mutableLabels.put("extra", "value");
        assertEquals(1, resource.labels().size());
        assertEquals("prod", resource.labels().get("env"));
    }

    @Test
    void nullLabelsBecomeEmptyMap() {
        ConfigResource resource = new ConfigResource(
                sampleId, ConfigKind.CLUSTER, new ConfigScope.Global(),
                1, now, now, "admin", null, new TestSpec("test"));

        assertTrue(resource.labels().isEmpty());
    }

    @Test
    void labelsMapIsUnmodifiable() {
        ConfigResource resource = sampleResource(Map.of("env", "prod"));

        assertThrows(UnsupportedOperationException.class,
                () -> resource.labels().put("new-key", "new-value"));
    }
}
