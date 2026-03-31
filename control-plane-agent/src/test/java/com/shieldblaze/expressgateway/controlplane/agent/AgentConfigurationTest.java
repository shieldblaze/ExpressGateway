package com.shieldblaze.expressgateway.controlplane.agent;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

class AgentConfigurationTest {

    @TempDir
    Path tempDir;

    @Test
    void plaintextConfigCreatesValidConfig() {
        AgentConfiguration config = AgentConfiguration.plaintext(
                "localhost", 9090, "node-1", "cluster-1", "prod",
                tempDir.resolve("lkg.json")
        );

        assertEquals("localhost", config.controlPlaneAddress());
        assertEquals(9090, config.controlPlanePort());
        assertEquals("node-1", config.nodeId());
        assertEquals("cluster-1", config.clusterId());
        assertEquals("prod", config.environment());
        assertFalse(config.tlsEnabled());
    }

    @Test
    void tlsConfigRequiresCertPaths() {
        assertThrows(NullPointerException.class, () -> new AgentConfiguration(
                "localhost", 9090, "node-1", "cluster-1", "prod",
                "127.0.0.1", "1.0.0", "token",
                tempDir.resolve("lkg.json"),
                true, null, null, null // TLS enabled but no cert paths
        ));
    }

    @Test
    void tlsConfigAcceptsCertPaths() {
        Path cert = tempDir.resolve("cert.pem");
        Path key = tempDir.resolve("key.pem");
        Path trust = tempDir.resolve("ca.pem");

        AgentConfiguration config = new AgentConfiguration(
                "localhost", 9090, "node-1", "cluster-1", "prod",
                "127.0.0.1", "1.0.0", "token",
                tempDir.resolve("lkg.json"),
                true, cert, key, trust
        );

        assertNotNull(config);
        assertEquals(cert, config.tlsCertPath());
        assertEquals(key, config.tlsKeyPath());
        assertEquals(trust, config.tlsTrustCertPath());
    }

    @Test
    void invalidPortThrows() {
        assertThrows(IllegalArgumentException.class, () -> AgentConfiguration.plaintext(
                "localhost", 0, "node-1", "cluster-1", "prod",
                tempDir.resolve("lkg.json")
        ));

        assertThrows(IllegalArgumentException.class, () -> AgentConfiguration.plaintext(
                "localhost", 70000, "node-1", "cluster-1", "prod",
                tempDir.resolve("lkg.json")
        ));
    }

    @Test
    void nullNodeIdThrows() {
        assertThrows(NullPointerException.class, () -> AgentConfiguration.plaintext(
                "localhost", 9090, null, "cluster-1", "prod",
                tempDir.resolve("lkg.json")
        ));
    }

    @Test
    void nullAddressThrows() {
        assertThrows(NullPointerException.class, () -> AgentConfiguration.plaintext(
                null, 9090, "node-1", "cluster-1", "prod",
                tempDir.resolve("lkg.json")
        ));
    }
}
