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
package com.shieldblaze.expressgateway.restapi.api.configuration;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
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
import org.springframework.web.bind.annotation.RestController;

import javax.net.ssl.SSLException;
import java.io.IOException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;

@RestController
@RequestMapping("/v1/configuration/tls/client")
public final class TLSClient {

    @PostMapping(consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> applyConfiguration(@RequestBody TLSConfiguration tlsConfiguration) throws IOException {
        tlsConfiguration.validate().saveClient();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    /**
     * @throws JsonProcessingException This should never happen
     */
    @GetMapping(value = "/default", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getDefaultConfiguration() throws JsonProcessingException {
        JsonObject tlsClient = JsonParser.parseString(ConfigurationMarshaller.get(TLSConfiguration.DEFAULT_CLIENT)).getAsJsonObject();

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("TLSClientConfiguration").withMessage(tlsClient).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @GetMapping(produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getConfiguration() {
        JsonObject tlsClient;
        try {
            TLSConfiguration tlsConfiguration = TLSConfiguration.loadClient();
            tlsClient = JsonParser.parseString(ConfigurationMarshaller.get(tlsConfiguration)).getAsJsonObject();
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.CONFIGURATION_NOT_FOUND, ex.getMessage(), HttpResponseStatus.NOT_FOUND);
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("TLSClientConfiguration").withMessage(tlsClient).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @PostMapping("/addMapping")
    public ResponseEntity<String> addMapping(@RequestBody CertificateKeyPairStruct certificateKeyPairStruct)
            throws NoSuchAlgorithmException, KeyStoreException, SSLException {
        TLSConfiguration tlsClient = TLSConfiguration.loadClient();
        tlsClient.addMapping(certificateKeyPairStruct.host(), CertificateKeyPair.forClient(certificateKeyPairStruct.x509Certificates(), certificateKeyPairStruct.privateKey()));

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @DeleteMapping("/removeMapping")
    public ResponseEntity<String> removeMapping(@RequestBody TLSMappingStruct tlsMappingStruct) {
        TLSConfiguration tlsClient = TLSConfiguration.loadClient();

        boolean isRemoved = tlsClient.removeMapping(tlsMappingStruct.host());

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
