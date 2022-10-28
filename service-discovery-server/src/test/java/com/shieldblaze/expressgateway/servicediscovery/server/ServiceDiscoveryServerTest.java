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
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.boot.test.context.SpringBootTest;
import org.springframework.boot.test.web.client.TestRestTemplate;
import org.springframework.boot.test.web.server.LocalServerPort;
import org.springframework.http.HttpMethod;
import org.springframework.http.RequestEntity;
import org.springframework.http.ResponseEntity;

import java.net.URI;

import static org.assertj.core.api.Assertions.assertThat;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
@SpringBootTest(webEnvironment = SpringBootTest.WebEnvironment.RANDOM_PORT, args = "localhost:2181")
class ServiceDiscoveryServerTest {

    private static final Node NODE = new Node("1-2-3-4-5-f", "127.0.0.1", 9110, false);

    @LocalServerPort
    private int ServerPort;

    @Autowired
    private TestRestTemplate restTemplate;

    @Autowired
    private Handler handler;

    @Order(1)
    @Test
    public void loadHandlerTest() {
        assertThat(handler).isNotNull();
    }

    @Order(2)
    @Test
    public void registerServiceValidateSuccessful() {
        RequestEntity<Node> request = new RequestEntity<>(NODE, HttpMethod.PUT, URI.create("http://localhost:" + ServerPort + "/api/v1/service/register"));

        ResponseEntity<String> response = restTemplate.exchange(request, String.class);
        assertThat(response.getBody()).isNotNull();
        assertThat(response.getStatusCodeValue()).isEqualTo(200);
    }

    @Order(3)
    @Test
    public void getServiceAndValidateSuccessful() {
        String result = restTemplate.getForObject("http://localhost:" + ServerPort + "/api/v1/service/get?id=1-2-3-4-5-f", String.class);
        JsonObject jsonObject = JsonParser.parseString(result).getAsJsonObject();

        assertThat(jsonObject.get("Success").getAsBoolean()).isTrue();
    }

    @Order(4)
    @Test
    public void getAllServicesAndValidateSuccessful() {
        String result = restTemplate.getForObject("http://localhost:" + ServerPort + "/api/v1/service/getall", String.class);

        JsonObject jsonObject = JsonParser.parseString(result).getAsJsonObject();
        assertThat(jsonObject.get("Success").getAsBoolean()).isTrue();
    }

    @Order(5)
    @Test
    public void unregisterServiceAndValidateSuccessful() {
        RequestEntity<Node> request = new RequestEntity<>(NODE, HttpMethod.DELETE, URI.create("http://localhost:" + ServerPort + "/api/v1/service/unregister"));

        ResponseEntity<String> response = restTemplate.exchange(request, String.class);
        assertThat(response.getBody()).isNotNull();
        assertThat(response.getStatusCodeValue()).isEqualTo(200);
    }
}
