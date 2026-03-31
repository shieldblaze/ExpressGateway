package com.shieldblaze.expressgateway.controlplane.config;

import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ConfigMutationTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private ConfigResource sampleResource() {
        return new ConfigResource(
                new ConfigResourceId("cluster", "global", "prod"),
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                Map.of(),
                new TestSpec("test")
        );
    }

    @Test
    void upsertWrapsResourceCorrectly() {
        ConfigResource resource = sampleResource();
        ConfigMutation.Upsert upsert = new ConfigMutation.Upsert(resource);
        assertEquals(resource, upsert.resource());
    }

    @Test
    void deleteWrapsResourceIdCorrectly() {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", "prod");
        ConfigMutation.Delete delete = new ConfigMutation.Delete(id);
        assertEquals(id, delete.resourceId());
    }

    @Test
    void upsertWithNullResourceThrows() {
        assertThrows(NullPointerException.class,
                () -> new ConfigMutation.Upsert(null));
    }

    @Test
    void deleteWithNullResourceIdThrows() {
        assertThrows(NullPointerException.class,
                () -> new ConfigMutation.Delete(null));
    }

    @Test
    void patternMatchingWithSwitch() {
        ConfigResource resource = sampleResource();
        ConfigMutation upsert = new ConfigMutation.Upsert(resource);
        ConfigMutation delete = new ConfigMutation.Delete(resource.id());

        String upsertResult = switch (upsert) {
            case ConfigMutation.Upsert u -> "upsert:" + u.resource().id().name();
            case ConfigMutation.Delete d -> "delete:" + d.resourceId().name();
        };
        assertEquals("upsert:prod", upsertResult);

        String deleteResult = switch (delete) {
            case ConfigMutation.Upsert u -> "upsert:" + u.resource().id().name();
            case ConfigMutation.Delete d -> "delete:" + d.resourceId().name();
        };
        assertEquals("delete:prod", deleteResult);
    }

    @Test
    void upsertIsInstanceOfConfigMutation() {
        ConfigMutation mutation = new ConfigMutation.Upsert(sampleResource());
        assertInstanceOf(ConfigMutation.Upsert.class, mutation);
    }

    @Test
    void deleteIsInstanceOfConfigMutation() {
        ConfigMutation mutation = new ConfigMutation.Delete(
                new ConfigResourceId("cluster", "global", "prod"));
        assertInstanceOf(ConfigMutation.Delete.class, mutation);
    }
}
