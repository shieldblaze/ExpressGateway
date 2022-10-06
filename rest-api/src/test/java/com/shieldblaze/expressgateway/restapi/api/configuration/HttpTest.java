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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.common.curator.Environment;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.restapi.CustomOkHttpClient;
import com.shieldblaze.expressgateway.restapi.RestApi;
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
class HttpTest {

    @BeforeAll
    static void startSpring() {
        Environment.setEnvironment(Environment.DEVELOPMENT);
        SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(List.of("127.0.0.1"), List.of("shieldblaze.com"));
        CryptoEntry cryptoEntry = new CryptoEntry(ssc.keyPair().getPrivate(), new X509Certificate[]{ssc.x509Certificate()});
        RestApi.start(cryptoEntry);
    }

    @AfterAll
    static void teardown() {
        RestApi.stop();
    }

    @Order(1)
    @Test
    void applyConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("maxContentLength", 1024 * 1024);
        jsonBody.addProperty("h2InitialWindowSize", 65535);
        jsonBody.addProperty("h2MaxConcurrentStreams", 1000);
        jsonBody.addProperty("h2MaxHeaderListSize", 262144);
        jsonBody.addProperty("h2MaxHeaderTableSize", 65536);
        jsonBody.addProperty("h2MaxFrameSize", 16777215);
        jsonBody.addProperty("maxInitialLineLength", 1024 * 8);
        jsonBody.addProperty("maxHeaderSize", 1024 * 8);
        jsonBody.addProperty("maxChunkSize", 1024 * 8);
        jsonBody.addProperty("compressionThreshold", 1024);
        jsonBody.addProperty("deflateCompressionLevel", 6);
        jsonBody.addProperty("brotliCompressionLevel", 4);

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/http")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            System.out.println(responseJson);
            assertTrue(responseJson.get("Success").getAsBoolean());
        }
    }

    @Order(2)
    @Test
    void applyBadConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("maxContentLength", 1024 * 1024);
        jsonBody.addProperty("h2InitialWindowSize", 65535);
        jsonBody.addProperty("h2MaxConcurrentStreams", 1000);
        jsonBody.addProperty("h2MaxHeaderListSize", 262144);
        jsonBody.addProperty("h2MaxHeaderTableSize", 65536);
        jsonBody.addProperty("h2MaxFrameSize", 16777215);
        jsonBody.addProperty("maxInitialLineLength", 1024 * 8);
        jsonBody.addProperty("maxHeaderSize", 1024 * 8);
        jsonBody.addProperty("maxChunkSize", 1024 * 8);
        jsonBody.addProperty("compressionThreshold", 1024);
        jsonBody.addProperty("deflateCompressionLevel", 6);
        jsonBody.addProperty("brotliCompressionLevel", 23); // Out of range

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/http")
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
        HttpConfiguration httpDefault = HttpConfiguration.DEFAULT;

        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/http/default")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("HttpConfiguration").getAsJsonObject();

            assertEquals(httpDefault.maxContentLength(), bufferObject.get("maxContentLength").getAsInt());
            assertEquals(httpDefault.h2InitialWindowSize(), bufferObject.get("h2InitialWindowSize").getAsInt());
            assertEquals(httpDefault.h2MaxConcurrentStreams(), bufferObject.get("h2MaxConcurrentStreams").getAsLong());
            assertEquals(httpDefault.h2MaxHeaderListSize(), bufferObject.get("h2MaxHeaderListSize").getAsLong());
            assertEquals(httpDefault.h2MaxHeaderTableSize(), bufferObject.get("h2MaxHeaderTableSize").getAsLong());
            assertEquals(httpDefault.h2MaxFrameSize(), bufferObject.get("h2MaxFrameSize").getAsInt());
            assertEquals(httpDefault.maxInitialLineLength(), bufferObject.get("maxInitialLineLength").getAsInt());
            assertEquals(httpDefault.maxHeaderSize(), bufferObject.get("maxHeaderSize").getAsInt());
            assertEquals(httpDefault.maxChunkSize(), bufferObject.get("maxChunkSize").getAsInt());
            assertEquals(httpDefault.compressionThreshold(), bufferObject.get("compressionThreshold").getAsInt());
            assertEquals(httpDefault.deflateCompressionLevel(), bufferObject.get("deflateCompressionLevel").getAsInt());
            assertEquals(httpDefault.brotliCompressionLevel(), bufferObject.get("brotliCompressionLevel").getAsInt());
        }
    }

    @Order(4)
    @Test
    void getConfiguration() throws IOException {
        JsonObject jsonBody = new JsonObject();
        jsonBody.addProperty("maxContentLength", 1024 * 1024);
        jsonBody.addProperty("h2InitialWindowSize", 65535);
        jsonBody.addProperty("h2MaxConcurrentStreams", 1000);
        jsonBody.addProperty("h2MaxHeaderListSize", 262144);
        jsonBody.addProperty("h2MaxHeaderTableSize", 65536);
        jsonBody.addProperty("h2MaxFrameSize", 16777215);
        jsonBody.addProperty("maxInitialLineLength", 1024 * 8);
        jsonBody.addProperty("maxHeaderSize", 1024 * 8);
        jsonBody.addProperty("maxChunkSize", 1024 * 8);
        jsonBody.addProperty("compressionThreshold", 1024);
        jsonBody.addProperty("deflateCompressionLevel", 6);
        jsonBody.addProperty("brotliCompressionLevel", 4);


        Request request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/http")
                .post(RequestBody.create(jsonBody.toString().getBytes()))
                .header("Content-Type", "application/json")
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
        }

        request = new Request.Builder()
                .url("https://127.0.0.1:9110/v1/configuration/http")
                .get()
                .build();

        try (Response response = CustomOkHttpClient.INSTANCE.newCall(request).execute()) {
            assertNotNull(response.body());
            JsonObject responseJson = JsonParser.parseString(response.body().string()).getAsJsonObject();
            assertTrue(responseJson.get("Success").getAsBoolean());
            JsonObject bufferObject = responseJson.get("Result").getAsJsonObject().get("HttpConfiguration").getAsJsonObject();

            assertEquals(jsonBody.get("maxContentLength").getAsInt(), bufferObject.get("maxContentLength").getAsInt());
            assertEquals(jsonBody.get("h2InitialWindowSize").getAsInt(), bufferObject.get("h2InitialWindowSize").getAsInt());
            assertEquals(jsonBody.get("h2MaxConcurrentStreams").getAsInt(), bufferObject.get("h2MaxConcurrentStreams").getAsLong());
            assertEquals(jsonBody.get("h2MaxHeaderListSize").getAsInt(), bufferObject.get("h2MaxHeaderListSize").getAsLong());
            assertEquals(jsonBody.get("h2MaxHeaderTableSize").getAsInt(), bufferObject.get("h2MaxHeaderTableSize").getAsLong());
            assertEquals(jsonBody.get("h2MaxFrameSize").getAsInt(), bufferObject.get("h2MaxFrameSize").getAsInt());
            assertEquals(jsonBody.get("maxInitialLineLength").getAsInt(), bufferObject.get("maxInitialLineLength").getAsInt());
            assertEquals(jsonBody.get("maxHeaderSize").getAsInt(), bufferObject.get("maxHeaderSize").getAsInt());
            assertEquals(jsonBody.get("maxChunkSize").getAsInt(), bufferObject.get("maxChunkSize").getAsInt());
            assertEquals(jsonBody.get("compressionThreshold").getAsInt(), bufferObject.get("compressionThreshold").getAsInt());
            assertEquals(jsonBody.get("deflateCompressionLevel").getAsInt(), bufferObject.get("deflateCompressionLevel").getAsInt());
            assertEquals(jsonBody.get("brotliCompressionLevel").getAsInt(), bufferObject.get("brotliCompressionLevel").getAsInt());
        }
    }
}
