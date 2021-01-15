/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

package com.shieldblaze.expressgateway.controlinterface.configuration;

import com.shieldblaze.expressgateway.controlinterface.tls.TLS;
import com.shieldblaze.expressgateway.controlinterface.tls.TLSClientServiceGrpc;
import com.shieldblaze.expressgateway.controlinterface.tls.TLSServerServiceGrpc;
import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import io.grpc.Server;
import io.grpc.ServerBuilder;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.security.cert.CertificateException;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

import static org.junit.jupiter.api.Assertions.*;

class TLSServerServiceTest {
    static Server server;

    @BeforeAll
    static void setup() throws IOException {
        System.setProperty("EGWConfDir", System.getProperty("java.io.tmpdir"));

        server = ServerBuilder.forPort(9110)
                .addService(new TLSServerService())
                .build()
                .start();
    }

    @AfterAll
    static void shutdown() {
        server.shutdownNow();
    }

    @Test
    void simpleTest() throws Exception {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        SelfSignedCertificate sscA = new SelfSignedCertificate("a.localhost", "EC", 256);
        SelfSignedCertificate sscB = new SelfSignedCertificate("b.localhost", "EC", 256);

        Map<String, TLS.Server.CertificateKeyPair> map = new ConcurrentHashMap<>();
        map.put("a.localhost", TLS.Server.CertificateKeyPair.newBuilder()
                .setCertificateChain(Files.readString(sscA.certificate().toPath()))
                .setPrivateKey(Files.readString(sscA.privateKey().toPath()))
                .setUseOCSP(false)
                .build());

        map.put("b.localhost", TLS.Server.CertificateKeyPair.newBuilder()
                .setCertificateChain(Files.readString(sscB.certificate().toPath()))
                .setPrivateKey(Files.readString(sscB.privateKey().toPath()))
                .setUseOCSP(false)
                .build());

        TLSServerServiceGrpc.TLSServerServiceBlockingStub tlsService = TLSServerServiceGrpc.newBlockingStub(channel);
        TLS.Server server = TLS.Server.newBuilder()
                .setUseStartTLS(true)
                .addProtocols("TLSv1.3")
                .addCiphers("TLS_AES_256_GCM_SHA384")
                .putAllTlsServerMapping(map)
                .setProfileName("Meow")
                .setPassword("MeowMeow")
                .build();

        TLS.ConfigurationResponse configurationResponse = tlsService.server(server);
        assertEquals(1, configurationResponse.getResponseCode());
        assertEquals("Success", configurationResponse.getResponseText());

        channel.shutdownNow();
    }


    @Test
    void failingTest() throws Exception {
        ManagedChannel channel = ManagedChannelBuilder.forTarget("127.0.0.1:9110")
                .usePlaintext()
                .build();

        SelfSignedCertificate sscA = new SelfSignedCertificate("a.localhost", "EC", 256);
        SelfSignedCertificate sscB = new SelfSignedCertificate("b.localhost", "EC", 256);

        Map<String, TLS.Server.CertificateKeyPair> map = new ConcurrentHashMap<>();
        map.put("a.localhost", TLS.Server.CertificateKeyPair.newBuilder()
                .setCertificateChain(Files.readString(sscA.certificate().toPath()))
                .setPrivateKey(Files.readString(sscA.privateKey().toPath()))
                .setUseOCSP(false)
                .build());

        map.put("b.localhost", TLS.Server.CertificateKeyPair.newBuilder()
                .setCertificateChain(Files.readString(sscB.certificate().toPath()))
                .setPrivateKey(Files.readString(sscB.privateKey().toPath()))
                .setUseOCSP(false)
                .build());

        TLSServerServiceGrpc.TLSServerServiceBlockingStub tlsService = TLSServerServiceGrpc.newBlockingStub(channel);
        TLS.Server server = TLS.Server.newBuilder()
                .setUseStartTLS(true)
                .addProtocols("TLSv1.3")
                .addCiphers("TLS_AES_256_GCM_SHA256")
                .putAllTlsServerMapping(map)
                .setProfileName("Meow")
                .setPassword("MeowMeow")
                .build();

        TLS.ConfigurationResponse configurationResponse = tlsService.server(server);
        assertEquals(-1, configurationResponse.getResponseCode());

        channel.shutdownNow();
    }
}
