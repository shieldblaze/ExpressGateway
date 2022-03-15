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
import com.shieldblaze.expressgateway.common.Gson;
import com.shieldblaze.expressgateway.common.JacksonJson;
import com.shieldblaze.expressgateway.configuration.ConfigurationStore;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.restapi.response.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.io.IOException;

@RestController
@RequestMapping("/v1/configuration/{profile}/healthcheck")
public final class HealthCheck {

    @PostMapping(value = "save", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> applyConfiguration(@RequestBody HealthCheckConfiguration healthCheck, @PathVariable String profile) throws IOException {
        if (profile.equalsIgnoreCase("default")) {
            return FastBuilder.error(ErrorBase.CONFIGURATION_NOT_FOUND, "Default configuration cannot be modified",
                    HttpResponseStatus.BAD_REQUEST);
        }

        healthCheck.validate();
        ConfigurationStore.save(profile, healthCheck);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @GetMapping(value = "/get", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getConfiguration(@PathVariable String profile) {
        JsonObject healthCheck;
        try {
            HealthCheckConfiguration healthCheckConfiguration;
            if (profile.equalsIgnoreCase("default")) {
                healthCheckConfiguration = HealthCheckConfiguration.DEFAULT;
            } else {
                healthCheckConfiguration = ConfigurationStore.load(profile, HealthCheckConfiguration.class);
            }
            healthCheck = JsonParser.parseString(JacksonJson.get(healthCheckConfiguration)).getAsJsonObject();
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.CONFIGURATION_NOT_FOUND, ex.getMessage(), HttpResponseStatus.NOT_FOUND);
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("HealthCheckConfiguration").withMessage(healthCheck).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }
}
