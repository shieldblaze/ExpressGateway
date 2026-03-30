package com.shieldblaze.expressgateway.controlplane.agent;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.google.protobuf.ByteString;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigResponse;
import com.shieldblaze.expressgateway.controlplane.v1.Resource;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Path;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigStreamHandlerTest {

    private static final ObjectMapper MAPPER = new ObjectMapper().findAndRegisterModules();

    /**
     * Serialize a ConfigSpec matching server-side behavior: plain JSON without type discriminator.
     * The agent resolves the concrete type from the proto type_url field.
     */
    private static byte[] serializeSpec(ConfigSpec spec) throws Exception {
        return MAPPER.writeValueAsBytes(spec);
    }

    @TempDir
    Path tempDir;

    private ConfigStreamHandler handler;
    private ConfigApplier applier;

    @BeforeEach
    void setUp() {
        applier = new ConfigApplier();
        LKGStore lkgStore = new LKGStore(tempDir.resolve("lkg.json"));
        handler = new ConfigStreamHandler("node-1", "token-1", applier, lkgStore);
    }

    @Test
    void deserializeUpsertMutations() throws Exception {
        ClusterSpec clusterSpec = new ClusterSpec("prod-backend", "round-robin",
                "http-check", 1000, 30);
        byte[] payload = serializeSpec(clusterSpec);

        ConfigResponse response = ConfigResponse.newBuilder()
                .setTypeUrl("cluster")
                .setVersion("42")
                .setNonce("test-nonce-1")
                .addResources(Resource.newBuilder()
                        .setName("prod-backend")
                        .setTypeUrl("cluster")
                        .setVersion("42")
                        .setPayload(ByteString.copyFrom(payload))
                        .build())
                .build();

        List<ConfigMutation> mutations = handler.deserializeMutations(response);

        assertEquals(1, mutations.size());
        assertInstanceOf(ConfigMutation.Upsert.class, mutations.get(0));

        ConfigMutation.Upsert upsert = (ConfigMutation.Upsert) mutations.get(0);
        ConfigResource resource = upsert.resource();
        assertEquals("prod-backend", resource.id().name());
        assertEquals("cluster", resource.kind().name());
        assertEquals(42, resource.version());
        assertInstanceOf(ClusterSpec.class, resource.spec());

        ClusterSpec deserializedSpec = (ClusterSpec) resource.spec();
        assertEquals("prod-backend", deserializedSpec.name());
        assertEquals("round-robin", deserializedSpec.loadBalanceStrategy());
        assertEquals("http-check", deserializedSpec.healthCheckName());
        assertEquals(1000, deserializedSpec.maxConnections());
    }

    @Test
    void deserializeDeleteMutations() throws Exception {
        ConfigResponse response = ConfigResponse.newBuilder()
                .setTypeUrl("cluster")
                .setVersion("43")
                .setNonce("test-nonce-2")
                .addRemovedResources("cluster/global/old-backend")
                .addRemovedResources("cluster/global/stale-backend")
                .build();

        List<ConfigMutation> mutations = handler.deserializeMutations(response);

        assertEquals(2, mutations.size());
        assertInstanceOf(ConfigMutation.Delete.class, mutations.get(0));
        assertInstanceOf(ConfigMutation.Delete.class, mutations.get(1));

        ConfigMutation.Delete delete1 = (ConfigMutation.Delete) mutations.get(0);
        assertEquals("cluster", delete1.resourceId().kind());
        assertEquals("global", delete1.resourceId().scopeQualifier());
        assertEquals("old-backend", delete1.resourceId().name());

        ConfigMutation.Delete delete2 = (ConfigMutation.Delete) mutations.get(1);
        assertEquals("stale-backend", delete2.resourceId().name());
    }

    @Test
    void deserializeMixedUpsertAndDelete() throws Exception {
        ClusterSpec clusterSpec = new ClusterSpec("new-cluster", "least-connection",
                "tcp-check", 500, 10);
        byte[] payload = serializeSpec(clusterSpec);

        ConfigResponse response = ConfigResponse.newBuilder()
                .setTypeUrl("cluster")
                .setVersion("50")
                .setNonce("test-nonce-3")
                .addResources(Resource.newBuilder()
                        .setName("new-cluster")
                        .setTypeUrl("cluster")
                        .setVersion("50")
                        .setPayload(ByteString.copyFrom(payload))
                        .build())
                .addRemovedResources("cluster/global/old-cluster")
                .build();

        List<ConfigMutation> mutations = handler.deserializeMutations(response);

        assertEquals(2, mutations.size());
        assertInstanceOf(ConfigMutation.Upsert.class, mutations.get(0));
        assertInstanceOf(ConfigMutation.Delete.class, mutations.get(1));
    }

    @Test
    void deserializeEmptyResponseProducesNoMutations() throws Exception {
        ConfigResponse response = ConfigResponse.newBuilder()
                .setTypeUrl("cluster")
                .setVersion("1")
                .setNonce("test-nonce-4")
                .build();

        List<ConfigMutation> mutations = handler.deserializeMutations(response);

        assertTrue(mutations.isEmpty());
    }

    @Test
    void deserializeUsesResponseTypeUrlWhenResourceTypeUrlEmpty() throws Exception {
        ClusterSpec clusterSpec = new ClusterSpec("fallback-cluster", "random",
                "health-1", 200, 5);
        byte[] payload = serializeSpec(clusterSpec);

        // Resource has empty type_url; should fall back to response-level type_url
        ConfigResponse response = ConfigResponse.newBuilder()
                .setTypeUrl("cluster")
                .setVersion("10")
                .setNonce("test-nonce-5")
                .addResources(Resource.newBuilder()
                        .setName("fallback-cluster")
                        .setVersion("10")
                        .setPayload(ByteString.copyFrom(payload))
                        .build())
                .build();

        List<ConfigMutation> mutations = handler.deserializeMutations(response);

        assertEquals(1, mutations.size());
        ConfigMutation.Upsert upsert = (ConfigMutation.Upsert) mutations.get(0);
        assertEquals("cluster", upsert.resource().kind().name());
    }

    @Test
    void deserializeMultipleResources() throws Exception {
        ClusterSpec spec1 = new ClusterSpec("cluster-a", "round-robin", "hc-1", 100, 10);
        ClusterSpec spec2 = new ClusterSpec("cluster-b", "least-connection", "hc-2", 200, 20);

        ConfigResponse response = ConfigResponse.newBuilder()
                .setTypeUrl("cluster")
                .setVersion("55")
                .setNonce("test-nonce-6")
                .addResources(Resource.newBuilder()
                        .setName("cluster-a")
                        .setTypeUrl("cluster")
                        .setVersion("55")
                        .setPayload(ByteString.copyFrom(serializeSpec(spec1)))
                        .build())
                .addResources(Resource.newBuilder()
                        .setName("cluster-b")
                        .setTypeUrl("cluster")
                        .setVersion("55")
                        .setPayload(ByteString.copyFrom(serializeSpec(spec2)))
                        .build())
                .build();

        List<ConfigMutation> mutations = handler.deserializeMutations(response);

        assertEquals(2, mutations.size());

        ConfigMutation.Upsert upsert1 = (ConfigMutation.Upsert) mutations.get(0);
        assertEquals("cluster-a", upsert1.resource().id().name());

        ConfigMutation.Upsert upsert2 = (ConfigMutation.Upsert) mutations.get(1);
        assertEquals("cluster-b", upsert2.resource().id().name());
    }
}
