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
import io.netty.util.internal.StringUtil;
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
    public ResponseEntity<String> getBuffer() throws Exception {
        return response(load(BufferConfiguration.class));
    }

    @GetMapping(value = "/buffer/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getBufferDefault() throws Exception {
        return response(BufferConfiguration.DEFAULT);
    }

    @PostMapping(value = "/buffer", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setBuffer(@RequestBody BufferConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/eventloop", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getEventLoop() throws Exception {
        return response(load(EventLoopConfiguration.class));
    }

    @GetMapping(value = "/eventloop/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getEventLoopDefault() throws Exception {
        return response(EventLoopConfiguration.DEFAULT);
    }

    @PostMapping(value = "/eventloop", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setEventLoop(@RequestBody EventLoopConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/eventstream", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getEventStream() throws Exception {
        return response(load(EventStreamConfiguration.class));
    }

    @GetMapping(value = "/eventstream/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getEventStreamDefault() throws Exception {
        return response(EventStreamConfiguration.DEFAULT);
    }

    @PostMapping(value = "/eventstream", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setEventStream(@RequestBody EventStreamConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/healthcheck", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getHealthCheck(@RequestParam(defaultValue = "default") String id) throws Exception {
        return response(load(HealthCheckConfiguration.class));
    }

    @GetMapping(value = "/healthcheck/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getHealthCheckDefault() throws Exception {
        return response(HealthCheckConfiguration.DEFAULT);
    }

    @PostMapping(value = "/healthcheck", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setHealthCheck(@RequestBody HealthCheckConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/http", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getHttp() throws Exception {
        return response(load(HttpConfiguration.class));
    }

    @GetMapping(value = "/http/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getHttpDefault() throws Exception {
        return response(HttpConfiguration.DEFAULT);
    }

    @PostMapping(value = "/http", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setHttp(@RequestBody HttpConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/transport", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTransport() throws Exception {
        return response(load(TransportConfiguration.class));
    }

    @GetMapping(value = "/transport/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTransportDefault() throws Exception {
        return response(TransportConfiguration.DEFAULT);
    }

    @PostMapping(value = "/transport", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setTransport(@RequestBody TransportConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/tls/server", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTlsServer() throws Exception {
        return response(load(TlsServerConfiguration.class));
    }

    @GetMapping(value = "/tls/server/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTlsServerDefault() throws Exception {
        return response(TlsServerConfiguration.DEFAULT);
    }

    @PostMapping(value = "/tls/server", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> setTlsServer(@RequestBody TlsServerConfiguration configuration) throws Exception {
        return saveAndGenerateResponse(configuration);
    }

    @GetMapping(value = "/tls/client", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTlsClient() throws Exception {
        return response(load(TlsClientConfiguration.class));
    }

    @GetMapping(value = "/tls/client/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getTlsClientDefault() throws Exception {
        return response(TlsClientConfiguration.DEFAULT);
    }

    @PostMapping(value = "/tls/client", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
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
        // Instantiate class from payload and validate it
        configuration.validate();

        // Save the class
        ConfigurationStore.save(configuration);

        // Build API response
        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader(StringUtil.simpleClassName(configuration.getClass())).withMessage("Saved").build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @PostMapping("/tls/client/mapping")
    public ResponseEntity<String> addClientMapping(@RequestBody CertificateKeyPairHolder certificateKeyPairHolder) throws Exception {
        TlsClientConfiguration tlsClient = load(TlsClientConfiguration.class);
        tlsClient.addMapping(certificateKeyPairHolder.host(),
                CertificateKeyPair.forClient(certificateKeyPairHolder.x509Certificates(), certificateKeyPairHolder.privateKey()));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @DeleteMapping("/tls/client/mapping")
    public ResponseEntity<String> removeClientMapping(@RequestBody TLSMappingHolder tlsMappingHolder) throws Exception {
        TlsClientConfiguration tlsClient = load(TlsClientConfiguration.class);
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
