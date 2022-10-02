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

import com.fasterxml.jackson.core.JsonProcessingException;
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
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import static com.shieldblaze.expressgateway.configuration.ConfigurationStore.load;

@RestController
@RequestMapping("/v1/configuration")
public final class ConfigurationEndpointHandler {

    @GetMapping(value = "/buffer", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getBuffer(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(BufferConfiguration.DEFAULT);
        } else {
            return response(load(id, BufferConfiguration.class));
        }
    }

    @PostMapping(value = "/buffer", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setBuffer(@RequestBody BufferConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/eventloop", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getEventLoop(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(EventLoopConfiguration.DEFAULT);
        } else {
            return response(load(id, EventLoopConfiguration.class));
        }
    }

    @PostMapping(value = "/eventloop", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setEventLoop(@RequestBody EventLoopConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/eventstream", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getEventStream(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(EventStreamConfiguration.DEFAULT);
        } else {
            return response(load(id, EventStreamConfiguration.class));
        }
    }

    @PostMapping(value = "/eventstream", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setEventStream(@RequestBody EventStreamConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/healthcheck", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getHealthCheck(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(HealthCheckConfiguration.DEFAULT);
        } else {
            return response(load(id, HealthCheckConfiguration.class));
        }
    }

    @PostMapping(value = "/healthcheck", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setHealthCheck(@RequestBody HealthCheckConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/http", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getHttp(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(HttpConfiguration.DEFAULT);
        } else {
            return response(load(id, HttpConfiguration.class));
        }
    }

    @PostMapping(value = "/http", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setHttp(@RequestBody HttpConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/transport", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTransport(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(TransportConfiguration.DEFAULT);
        } else {
            return response(load(id, TransportConfiguration.class));
        }
    }

    @PostMapping(value = "/transport", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setTransport(@RequestBody TransportConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/tlsserver", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTlsServer(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(TlsServerConfiguration.DEFAULT);
        } else {
            return response(load(id, TlsServerConfiguration.class));
        }
    }

    @PostMapping(value = "/tlsserver", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setTlsServer(@RequestBody TlsServerConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/tlsclient", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTlsClient(@RequestParam(defaultValue = "default") String id) throws Exception {
        if (id.equalsIgnoreCase("default")) {
            return response(TlsClientConfiguration.DEFAULT);
        } else {
            return response(load(id, TlsClientConfiguration.class));
        }
    }

    @PostMapping(value = "/tlsclient", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setTlsClient(@RequestBody TlsClientConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    private static ResponseEntity<String> response(Configuration<?> configuration) throws JsonProcessingException {
        JsonObject json = JsonParser.parseString(JacksonJson.get(configuration)).getAsJsonObject();
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader(configuration.getClass().getSimpleName()).withMessage(json).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    private static ResponseEntity<String> saveAndGenerateResponse(Configuration<?> configuration) throws Exception {
        // Default configuration cannot be modified
        if (configuration.id() != null && configuration.id().equalsIgnoreCase("default")) {
            return FastBuilder.error(ErrorBase.DEFAULT_CONFIGURATION_CANNOT_BE_MODIFIED, HttpResponseStatus.NOT_MODIFIED);
        }

        // Instantiate class from payload and validate it
        configuration.validate();

        // Save the class
        ConfigurationStore.save(configuration);

        // Build API response
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("ID").withMessage(configuration.id()).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @PostMapping("/tls/addmapping")
    public ResponseEntity<String> addMapping(@RequestParam String id, @RequestBody CertificateKeyPairHolder certificateKeyPairHolder) throws Exception {
        TlsClientConfiguration tlsClient = load(id, TlsClientConfiguration.class);
        tlsClient.addMapping(certificateKeyPairHolder.host(),
                CertificateKeyPair.forClient(certificateKeyPairHolder.x509Certificates(), certificateKeyPairHolder.privateKey()));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @DeleteMapping("/tls/removemapping")
    public ResponseEntity<String> removeMapping(@RequestBody TLSMappingHolder tlsMappingHolder, @RequestParam String id) throws Exception {
        TlsClientConfiguration tlsClient = load(id, TlsClientConfiguration.class);
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
