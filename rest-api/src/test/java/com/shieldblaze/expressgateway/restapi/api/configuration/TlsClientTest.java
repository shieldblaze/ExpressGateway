/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.restapi.api.configuration;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.common.curator.Environment;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsConfiguration;
import com.shieldblaze.expressgateway.restapi.CustomOkHttpClient;
import com.shieldblaze.expressgateway.restapi.RestApi;
import com.shieldblaze.expressgateway.testing.ExpressGatewayConfigured;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import java.io.IOException;
import java.security.cert.X509Certificate;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class TlsClientTest {

    @BeforeAll
    static void startSpring() throws IOException {
        ExpressGateway expressGateway = ExpressGatewayConfigured.forTest();
        ExpressGateway.setInstance(expressGateway);

        Curator.init();
        RestApi.start();
    }

    @AfterAll
    static void teardown() {
        RestApi.stop();
    }

    @Order(1)
    @Test
    void applyConfiguration() throws IOException {
        JsonArray ciphers = new JsonArray();
        ciphers.add(Cipher.TLS_AES_256_GCM_SHA384.toString());
        ciphers.add(Cipher.TLS_AES_128_GCM_SHA256.toString());
        ciphers.add(Cipher.TLS_CHACHA20_POLY1305_SHA256.toString());

        JsonArray protocols = new JsonArray();
        protocols.add(Protocol.TLS_1_2.toString());
        protocols.add(Protocol.TLS_1_3.toString());

        JsonObject jsonBody = new JsonObject();
        jsonBody.add("ciphers", ciphers);
        jsonBody.add("protocols", protocols);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/tls/client")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Order(2)
    @Test
    void applyBadConfiguration() throws IOException {
        JsonArray ciphers = new JsonArray();
        ciphers.add(Cipher.TLS_AES_256_GCM_SHA384.toString());
        ciphers.add(Cipher.TLS_AES_128_GCM_SHA256.toString());
        ciphers.add(Cipher.TLS_CHACHA20_POLY1305_SHA256.toString());

        JsonArray protocols = new JsonArray();

        JsonObject jsonBody = new JsonObject();
        jsonBody.add("ciphers", ciphers);
        jsonBody.add("protocols", protocols);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/tls/client")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertFalse(responseJson.get("Success").getAsBoolean());
        }
    }

    @Order(3)
    @Test
    void getDefaultConfiguration() throws IOException {
        TlsConfiguration clientDefault = TlsClientConfiguration.DEFAULT;

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/tls/client/default")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("TlsClientConfiguration").getAsJsonObject();

            JsonArray ciphers = new JsonArray();
            for (Cipher cipher : clientDefault.ciphers()) {
                ciphers.add(cipher.toString());
            }

            JsonArray protocols = new JsonArray();
            for (Protocol protocol : clientDefault.protocols()) {
                protocols.add(protocol.toString());
            }

            assertEquals(ciphers, bufferObject.get("ciphers").getAsJsonArray());
            assertEquals(protocols, bufferObject.get("protocols").getAsJsonArray());
            assertEquals(clientDefault.mutualTLS().toString(), bufferObject.get("mutualTLS").getAsString());
        }
    }

    @Order(4)
    @Test
    void getConfiguration() throws IOException {
        JsonArray ciphers = new JsonArray();
        ciphers.add(Cipher.TLS_AES_256_GCM_SHA384.toString());
        ciphers.add(Cipher.TLS_AES_128_GCM_SHA256.toString());
        ciphers.add(Cipher.TLS_CHACHA20_POLY1305_SHA256.toString());

        JsonArray protocols = new JsonArray();
        protocols.add(Protocol.TLS_1_2.toString());
        protocols.add(Protocol.TLS_1_3.toString());

        JsonObject jsonBody = new JsonObject();
        jsonBody.add("ciphers", ciphers);
        jsonBody.add("protocols", protocols);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/tls/client")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/tls/client/")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("TlsClientConfiguration").getAsJsonObject();

            assertEquals(jsonBody.get("ciphers").getAsJsonArray(), bufferObject.get("ciphers").getAsJsonArray());
            assertEquals(jsonBody.get("protocols").getAsJsonArray(), bufferObject.get("protocols").getAsJsonArray());
        }
    }
}
