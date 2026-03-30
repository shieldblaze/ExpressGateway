package com.shieldblaze.expressgateway.controlplane.config;

import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ConfigTransactionTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private ConfigResource sampleResource(String name) {
        return new ConfigResource(
                new ConfigResourceId("cluster", "global", name),
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
    void builderCreatesTransactionWithMutations() {
        ConfigResource r1 = sampleResource("res-1");
        ConfigResource r2 = sampleResource("res-2");

        ConfigTransaction tx = ConfigTransaction.builder("admin@example.com")
                .upsert(r1)
                .upsert(r2)
                .build();

        assertEquals(2, tx.mutations().size());
        assertEquals("admin@example.com", tx.author());
    }

    @Test
    void authorIsStoredCorrectly() {
        ConfigTransaction tx = ConfigTransaction.builder("ci-bot")
                .upsert(sampleResource("res-1"))
                .build();

        assertEquals("ci-bot", tx.author());
    }

    @Test
    void descriptionIsOptionalAndCanBeNull() {
        ConfigTransaction tx = ConfigTransaction.builder("admin")
                .upsert(sampleResource("res-1"))
                .build();

        assertNull(tx.description());
    }

    @Test
    void descriptionIsStoredWhenProvided() {
        ConfigTransaction tx = ConfigTransaction.builder("admin")
                .description("Add production cluster")
                .upsert(sampleResource("res-1"))
                .build();

        assertEquals("Add production cluster", tx.description());
    }

    @Test
    void emptyMutationsThrowsOnBuild() {
        ConfigTransaction.Builder builder = ConfigTransaction.builder("admin");
        assertThrows(IllegalStateException.class, builder::build);
    }

    @Test
    void blankAuthorThrows() {
        assertThrows(IllegalArgumentException.class,
                () -> ConfigTransaction.builder(""));
        assertThrows(IllegalArgumentException.class,
                () -> ConfigTransaction.builder("   "));
    }

    @Test
    void nullAuthorThrows() {
        assertThrows(NullPointerException.class,
                () -> ConfigTransaction.builder(null));
    }

    @Test
    void mutationsListIsImmutable() {
        ConfigTransaction tx = ConfigTransaction.builder("admin")
                .upsert(sampleResource("res-1"))
                .build();

        assertThrows(UnsupportedOperationException.class,
                () -> tx.mutations().add(new ConfigMutation.Upsert(sampleResource("res-2"))));
    }

    @Test
    void upsertAndDeleteCanBeMixed() {
        ConfigResource resource = sampleResource("res-1");
        ConfigResourceId deleteId = new ConfigResourceId("cluster", "global", "old-res");

        ConfigTransaction tx = ConfigTransaction.builder("admin")
                .upsert(resource)
                .delete(deleteId)
                .build();

        assertEquals(2, tx.mutations().size());

        ConfigMutation first = tx.mutations().get(0);
        ConfigMutation second = tx.mutations().get(1);

        assertEquals(ConfigMutation.Upsert.class, first.getClass());
        assertEquals(ConfigMutation.Delete.class, second.getClass());

        assertEquals(resource, ((ConfigMutation.Upsert) first).resource());
        assertEquals(deleteId, ((ConfigMutation.Delete) second).resourceId());
    }
}
