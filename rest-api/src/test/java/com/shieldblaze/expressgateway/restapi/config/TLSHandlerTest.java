/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.restapi.config;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.springframework.boot.SpringApplication;
import org.springframework.context.ConfigurableApplicationContext;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.cert.CertificateException;

import static org.junit.jupiter.api.Assertions.*;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class TLSHandlerTest {

    final static HttpClient HTTP_CLIENT = HttpClient.newHttpClient();
    static ConfigurableApplicationContext ctx;
    static SelfSignedCertificate selfSignedCertificate;

    @BeforeAll
    static void setup() {
        System.setProperty("egw.config.dir", System.getProperty("java.io.tmpdir"));
        ctx = SpringApplication.run(Server.class);
    }

    @AfterAll
    static void teardown() {
        ctx.close();
    }

    @Test
    @Order(1)
    void createServer() throws IOException, InterruptedException, CertificateException {
        selfSignedCertificate = new SelfSignedCertificate();

        JsonObject configJson = new JsonObject();
        configJson.addProperty("forServer", true);

        JsonObject certKeyPairMap = new JsonObject();

        JsonObject defaultHost = new JsonObject();
        defaultHost.addProperty("certificateChain", selfSignedCertificate.certificate().getAbsolutePath());
        defaultHost.addProperty("privateKey", selfSignedCertificate.privateKey().getAbsolutePath());
        defaultHost.addProperty("useOCSPStapling", false);
        certKeyPairMap.add("DEFAULT_HOST", defaultHost);
        configJson.add("certificateKeyPairMap", certKeyPairMap);

        JsonArray ciphers = new JsonArray();
        ciphers.add("TLS_AES_256_GCM_SHA384");
        ciphers.add("TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384");
        configJson.add("ciphers", ciphers);

        JsonArray protocols = new JsonArray();
        protocols.add("TLS_1_3");
        protocols.add("TLS_1_2");
        configJson.add("protocols", protocols);

        configJson.addProperty("mutualTLS", "NOT_REQUIRED");
        configJson.addProperty("useStartTLS", false);
        configJson.addProperty("sessionTimeout", 1000);
        configJson.addProperty("sessionCacheSize", 60);
        configJson.addProperty("acceptAllCerts", true);

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(configJson.toString()))
                .uri(URI.create("http://127.0.0.1:9110/config/tlsServer"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(204, httpResponse.statusCode());
    }

    @Test
    @Order(2)
    void getServer() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://127.0.0.1:9110/config/tlsServer"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();

        assertTrue(jsonObject.get("forServer").getAsBoolean());
        JsonObject certKeyMap = jsonObject.get("certificateKeyPairMap").getAsJsonObject();

        assertEquals(selfSignedCertificate.certificate().getAbsolutePath(), certKeyMap.get("DEFAULT_HOST").getAsJsonObject().get("certificateChain").getAsString());
        assertEquals(selfSignedCertificate.privateKey().getAbsolutePath(), certKeyMap.get("DEFAULT_HOST").getAsJsonObject().get("privateKey").getAsString());
        assertFalse(certKeyMap.get("DEFAULT_HOST").getAsJsonObject().get("useOCSPStapling").getAsBoolean());

        JsonArray ciphers = jsonObject.get("ciphers").getAsJsonArray();
        assertEquals("TLS_AES_256_GCM_SHA384", ciphers.get(0).getAsString());
        assertEquals("TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384", ciphers.get(1).getAsString());

        JsonArray protocols = jsonObject.get("protocols").getAsJsonArray();
        assertEquals("TLS_1_3", protocols.get(0).getAsString());
        assertEquals("TLS_1_2", protocols.get(1).getAsString());

        assertEquals("NOT_REQUIRED", jsonObject.get("mutualTLS").getAsString());
        assertFalse(jsonObject.get("useStartTLS").getAsBoolean());
        assertEquals(1000, jsonObject.get("sessionTimeout").getAsInt());
        assertEquals(60, jsonObject.get("sessionCacheSize").getAsInt());
        assertTrue(jsonObject.get("acceptAllCerts").getAsBoolean());
    }

    @Test
    @Order(3)
    void deleteServer() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .DELETE()
                .uri(URI.create("http://127.0.0.1:9110/config/tlsServer"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(204, httpResponse.statusCode());
    }

    @Test
    @Order(4)
    void testDeleteServer() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://127.0.0.1:9110/config/tlsServer"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(404, httpResponse.statusCode());
    }
}
