package com.shieldblaze.expressgateway.controlplane.agent;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ListenerSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RateLimitSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TransportSpec;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigApplierTest {

    private ConfigApplier applier;

    @BeforeEach
    void setUp() {
        applier = new ConfigApplier();
    }

    private static ConfigResource createResource(String kind, String name, ConfigSpec spec) {
        ConfigResourceId id = new ConfigResourceId(kind, "global", name);
        return new ConfigResource(
                id,
                new ConfigKind(kind, 1),
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                Map.of(),
                spec
        );
    }

    @Test
    void applyUpsertDelegatesToRegisteredHandler() {
        AtomicReference<ConfigResource> captured = new AtomicReference<>();
        applier.registerUpsertHandler("test-kind", captured::set);

        ConfigSpec spec = new TestSpec("hello");
        ConfigResource resource = createResource("test-kind", "my-resource", spec);

        applier.apply(List.of(new ConfigMutation.Upsert(resource)));

        assertNotNull(captured.get());
        assertEquals("my-resource", captured.get().id().name());
    }

    @Test
    void applyDeleteDelegatesToRegisteredHandler() {
        AtomicReference<ConfigResourceId> captured = new AtomicReference<>();
        applier.registerDeleteHandler("test-kind", captured::set);

        ConfigResourceId id = new ConfigResourceId("test-kind", "global", "my-resource");

        applier.apply(List.of(new ConfigMutation.Delete(id)));

        assertNotNull(captured.get());
        assertEquals("my-resource", captured.get().name());
    }

    @Test
    void applyMultipleMutationsInOrder() {
        List<String> executionOrder = new ArrayList<>();
        applier.registerUpsertHandler("test-kind", r -> executionOrder.add("upsert:" + r.id().name()));
        applier.registerDeleteHandler("test-kind", id -> executionOrder.add("delete:" + id.name()));

        ConfigSpec spec = new TestSpec("data");
        ConfigResource r1 = createResource("test-kind", "first", spec);
        ConfigResource r2 = createResource("test-kind", "second", spec);
        ConfigResourceId deleteId = new ConfigResourceId("test-kind", "global", "third");

        applier.apply(List.of(
                new ConfigMutation.Upsert(r1),
                new ConfigMutation.Upsert(r2),
                new ConfigMutation.Delete(deleteId)
        ));

        assertEquals(List.of("upsert:first", "upsert:second", "delete:third"), executionOrder);
    }

    @Test
    void handlerExceptionDoesNotStopSubsequentMutations() {
        AtomicInteger successCount = new AtomicInteger();
        applier.registerUpsertHandler("test-kind", r -> {
            if (r.id().name().equals("fail")) {
                throw new RuntimeException("intentional failure");
            }
            successCount.incrementAndGet();
        });

        ConfigSpec spec = new TestSpec("data");
        applier.apply(List.of(
                new ConfigMutation.Upsert(createResource("test-kind", "ok-1", spec)),
                new ConfigMutation.Upsert(createResource("test-kind", "fail", spec)),
                new ConfigMutation.Upsert(createResource("test-kind", "ok-2", spec))
        ));

        assertEquals(2, successCount.get(), "Both non-failing mutations should succeed");
    }

    @Test
    void unknownKindDoesNotThrow() {
        ConfigSpec spec = new TestSpec("data");
        ConfigResource resource = createResource("unknown-kind", "res", spec);

        // Should not throw - just log a warning
        applier.apply(List.of(new ConfigMutation.Upsert(resource)));
    }

    @Test
    void customHandlerOverridesBuiltIn() {
        AtomicReference<String> captured = new AtomicReference<>();
        applier.registerUpsertHandler("cluster", r -> captured.set("custom:" + r.id().name()));

        ClusterSpec spec = new ClusterSpec("my-cluster", "round-robin", "hc-1", 100, 10);
        ConfigResource resource = createResource("cluster", "my-cluster", spec);

        applier.apply(List.of(new ConfigMutation.Upsert(resource)));

        assertEquals("custom:my-cluster", captured.get());
    }

    @Test
    void builtInClusterHandlerValidatesSpec() {
        // Built-in handler should validate the spec and log success
        ClusterSpec spec = new ClusterSpec("valid-cluster", "round-robin", "hc", 500, 30);
        ConfigResource resource = createResource("cluster", "valid-cluster", spec);

        // Should not throw (no load balancers registered, but applies cleanly)
        applier.apply(List.of(new ConfigMutation.Upsert(resource)));
    }

    @Test
    void builtInTransportHandlerValidatesSpec() {
        TransportSpec spec = new TransportSpec("default-transport", "epoll", 65536, 65536, true, false);
        ConfigResource resource = createResource("transport", "default-transport", spec);

        // Should not throw
        applier.apply(List.of(new ConfigMutation.Upsert(resource)));
    }

    @Test
    void builtInRateLimitHandlerValidatesSpec() {
        RateLimitSpec spec = new RateLimitSpec("api-limiter", 100, 200, "global", null);
        ConfigResource resource = createResource("rate-limit", "api-limiter", spec);

        // Should not throw
        applier.apply(List.of(new ConfigMutation.Upsert(resource)));
    }

    @Test
    void deleteForUnregisteredKindDoesNotThrow() {
        ConfigResourceId id = new ConfigResourceId("nonexistent", "global", "res");
        applier.apply(List.of(new ConfigMutation.Delete(id)));
    }

    @Test
    void emptyMutationListIsNoOp() {
        applier.apply(List.of());
    }

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() {
            // no-op
        }
    }
}
