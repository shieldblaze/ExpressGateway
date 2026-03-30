package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;

import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigDeltaTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private ConfigMutation sampleUpsert(String name) {
        ConfigResource resource = new ConfigResource(
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
        return new ConfigMutation.Upsert(resource);
    }

    @Test
    void emptyDeltaIsEmpty() {
        ConfigDelta delta = new ConfigDelta(0, 1, List.of());
        assertTrue(delta.isEmpty());
    }

    @Test
    void nonEmptyDeltaIsNotEmpty() {
        ConfigDelta delta = new ConfigDelta(0, 1, List.of(sampleUpsert("res-1")));
        assertFalse(delta.isEmpty());
    }

    @Test
    void mutationsListIsImmutable() {
        ArrayList<ConfigMutation> mutableList = new ArrayList<>();
        mutableList.add(sampleUpsert("res-1"));

        ConfigDelta delta = new ConfigDelta(0, 1, mutableList);

        // Mutating the original list should not affect the delta
        mutableList.add(sampleUpsert("res-2"));
        assertEquals(1, delta.mutations().size());

        // The returned list itself should be unmodifiable
        assertThrows(UnsupportedOperationException.class,
                () -> delta.mutations().add(sampleUpsert("res-3")));
    }

    @Test
    void nullMutationsThrows() {
        assertThrows(NullPointerException.class,
                () -> new ConfigDelta(0, 1, null));
    }

    @Test
    void revisionsAreStoredCorrectly() {
        ConfigDelta delta = new ConfigDelta(5, 10, List.of());
        assertEquals(5, delta.fromRevision());
        assertEquals(10, delta.toRevision());
    }
}
