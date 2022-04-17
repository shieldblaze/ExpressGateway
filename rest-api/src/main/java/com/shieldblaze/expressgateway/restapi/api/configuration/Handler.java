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
import com.shieldblaze.expressgateway.common.JacksonJson;
import com.shieldblaze.expressgateway.configuration.Configuration;
import com.shieldblaze.expressgateway.configuration.ConfigurationStore;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TlsClientConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TlsServerConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.restapi.response.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.io.IOException;

@RestController
@RequestMapping("/v1/configuration")
public final class Handler {

    /**
     * Get a configuration
     *
     * @param config Configuration name
     * @param id     Configuration ID
     * @return {@link ResponseEntity} of response
     * @throws IOException In case of an exception
     */
    @GetMapping(value = "/{config}/get", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getConfiguration(@PathVariable String config,
                                                   @RequestParam String id) throws IOException {
        Configuration<?> configuration;
        if (config.equalsIgnoreCase("buffer")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = BufferConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, BufferConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("eventloop")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = EventLoopConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, EventLoopConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("eventstream")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = EventStreamConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, EventStreamConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("healthcheck")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = HealthCheckConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, HealthCheckConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("Http")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = HttpConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, HttpConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("TlsClient")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = TlsClientConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, TlsClientConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("TlsServer")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = TlsServerConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, TlsServerConfiguration.class);
            }
        } else if (config.equalsIgnoreCase("Transport")) {
            if (id.equalsIgnoreCase("default")) {
                configuration = TransportConfiguration.DEFAULT;
            } else {
                configuration = ConfigurationStore.load(id, TransportConfiguration.class);
            }
        } else {
            throw new IllegalArgumentException("Unknown Configuration: " + config);
        }

        JsonObject json = JsonParser.parseString(JacksonJson.get(configuration)).getAsJsonObject();
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader(configuration.getClass().getSimpleName()).withMessage(json).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    /**
     * Apply (or save) a configuration.
     *
     * @param id          ID is required when we're updating an existing configuration.
     *                    When we're creating a new configuration then 'ID' should be 'null'.
     * @param profileName Profile name always required when creating or modifying configuration.
     * @param config      Configuration name
     * @param requestBody Configuration body
     * @return {@link ResponseEntity} of response
     * @throws IOException In case of an exception
     */
    @PostMapping(value = "/{config}/save", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> applyConfiguration(@RequestParam(required = false) String id,
                                                     @RequestParam String profileName,
                                                     @PathVariable String config,
                                                     @RequestBody String requestBody) throws IOException {
        // Default configuration cannot be modified
        if (id != null && (id.equalsIgnoreCase("default") || profileName.equalsIgnoreCase("default"))) {
            return FastBuilder.error(ErrorBase.DEFAULT_CONFIGURATION_CANNOT_BE_MODIFIED, HttpResponseStatus.NOT_MODIFIED);
        }

        Configuration<?> configuration;
        if (config.equalsIgnoreCase("buffer")) {
            configuration = JacksonJson.read(requestBody, BufferConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("eventloop")) {
            configuration = JacksonJson.read(requestBody, EventLoopConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("eventstream")) {
            configuration = JacksonJson.read(requestBody, EventStreamConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("healthcheck")) {
            configuration = JacksonJson.read(requestBody, HealthCheckConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("Http")) {
            configuration = JacksonJson.read(requestBody, HttpConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("TlsClient")) {
            configuration = JacksonJson.read(requestBody, TlsClientConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("TlsServer")) {
            configuration = JacksonJson.read(requestBody, TlsServerConfiguration.class).setProfileName(profileName);
        } else if (config.equalsIgnoreCase("Transport")) {
            configuration = JacksonJson.read(requestBody, TransportConfiguration.class).setProfileName(profileName);
        } else {
            throw new IllegalArgumentException("Unknown Configuration: " + config);
        }

        configuration.validate(); // This will validate the Configuration and generate ID if necessary
        ConfigurationStore.save(configuration.id(), configuration);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("ID").withMessage(configuration.id()).build())
                .build();

        if (id == null) {
            return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.CREATED);
        } else {
            return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
        }
    }

    @PostMapping("/tls/addMapping")
    public ResponseEntity<String> addMapping(@RequestParam String id,
                                             @RequestBody CertificateKeyPairHolder certificateKeyPairHolder) throws IOException {
        TlsClientConfiguration tlsClient = ConfigurationStore.load(id, TlsClientConfiguration.class);
        tlsClient.addMapping(certificateKeyPairHolder.host(),
                CertificateKeyPair.forClient(certificateKeyPairHolder.x509Certificates(), certificateKeyPairHolder.privateKey()));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @DeleteMapping("/tls/removeMapping")
    public ResponseEntity<String> removeMapping(@RequestBody TLSMappingHolder tlsMappingHolder,
                                                @RequestParam String id) throws IOException {
        TlsClientConfiguration tlsClient = ConfigurationStore.load(id, TlsClientConfiguration.class);
        boolean isRemoved = tlsClient.removeMapping(tlsMappingHolder.host());

        APIResponse apiResponse;
        if (isRemoved) {
            apiResponse = APIResponse.newBuilder()
                    .isSuccess(true)
                    .build();

            return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
        } else {
            return FastBuilder.error(ErrorBase.INVALID_REQUEST, "Mapping not found", HttpResponseStatus.NOT_FOUND);
        }
    }
}
