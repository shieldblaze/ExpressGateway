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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.apache.curator.test.TestingServer;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.test.context.SpringBootTest;
import org.springframework.boot.test.web.server.LocalServerPort;
import org.springframework.http.HttpMethod;
import org.springframework.http.RequestEntity;
import org.springframework.http.ResponseEntity;
import org.springframework.http.client.SimpleClientHttpRequestFactory;
import org.springframework.web.client.RestTemplate;

import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509TrustManager;
import java.io.File;
import java.io.IOException;
import java.net.URI;
import java.security.cert.X509Certificate;

import static org.assertj.core.api.Assertions.assertThat;

@SpringBootTest(webEnvironment = SpringBootTest.WebEnvironment.DEFINED_PORT)
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class ServiceDiscoveryServerHttpsTest {

    private static final Node NODE = new Node("1-2-3-4-5-f", "127.0.0.1", 9110, false);
    private static TestingServer zooKeeperServer;

    static {
        ClassLoader classLoader = ServiceDiscoveryServerHttpsTest.class.getClassLoader();
        File file = new File(classLoader.getResource("secureConfiguration.json").getFile());
        String absolutePath = file.getAbsolutePath();

        System.setProperty("config.file", absolutePath);
    }

    private final RestTemplate restTemplate = createTrustAllRestTemplate();

    @LocalServerPort
    private int ServerPort;

    @Autowired
    private Handler handler;

    @BeforeAll
    static void setup() throws Exception {
        zooKeeperServer = new TestingServer(9002);
    }

    @AfterAll
    static void shutdown() throws IOException {
        System.clearProperty("config.file");
        if (zooKeeperServer != null) {
            zooKeeperServer.close();
        }
    }

    private static RestTemplate createTrustAllRestTemplate() {
        try {
            TrustManager[] trustAllCerts = new TrustManager[]{
                    new X509TrustManager() {
                        @Override
                        public X509Certificate[] getAcceptedIssuers() { return null; }
                        @Override
                        public void checkClientTrusted(X509Certificate[] certs, String authType) { }
                        @Override
                        public void checkServerTrusted(X509Certificate[] certs, String authType) { }
                    }
            };
            SSLContext sslContext = SSLContext.getInstance("TLS");
            sslContext.init(null, trustAllCerts, new java.security.SecureRandom());
            HttpsURLConnection.setDefaultSSLSocketFactory(sslContext.getSocketFactory());
            HttpsURLConnection.setDefaultHostnameVerifier((hostname, session) -> true);
            return new RestTemplate(new SimpleClientHttpRequestFactory());
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    @Order(1)
    @Test
    public void loadHandlerTest() {
        assertThat(handler).isNotNull();
    }

    @Order(2)
    @Test
    public void registerServiceValidateSuccessful() {
        RequestEntity<Node> request = new RequestEntity<>(NODE, HttpMethod.PUT, URI.create("https://localhost:" + ServerPort + "/api/v1/service/register"));

        ResponseEntity<String> response = restTemplate.exchange(request, String.class);
        assertThat(response.getBody()).isNotNull();
        assertThat(response.getStatusCode().value()).isEqualTo(200);
    }

    @Order(3)
    @Test
    public void getServiceAndValidateSuccessful() {
        String result = restTemplate.getForObject("https://localhost:" + ServerPort + "/api/v1/service/get?id=1-2-3-4-5-f", String.class);
        JsonObject jsonObject = JsonParser.parseString(result).getAsJsonObject();

        assertThat(jsonObject.get("Success").getAsBoolean()).isTrue();
    }

    @Order(4)
    @Test
    public void getAllServicesAndValidateSuccessful() {
        String result = restTemplate.getForObject("https://localhost:" + ServerPort + "/api/v1/service/getall", String.class);

        JsonObject jsonObject = JsonParser.parseString(result).getAsJsonObject();
        assertThat(jsonObject.get("Success").getAsBoolean()).isTrue();
    }

    @Order(5)
    @Test
    public void deregisterServiceAndValidateSuccessful() {
        RequestEntity<Node> request = new RequestEntity<>(NODE, HttpMethod.DELETE, URI.create("https://localhost:" + ServerPort + "/api/v1/service/deregister"));

        ResponseEntity<String> response = restTemplate.exchange(request, String.class);
        assertThat(response.getBody()).isNotNull();
        assertThat(response.getStatusCode().value()).isEqualTo(200);
    }
}
