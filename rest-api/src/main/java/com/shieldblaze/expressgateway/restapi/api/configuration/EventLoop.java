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
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.restapi.response.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.APIResponse;
import com.shieldblaze.expressgateway.restapi.response.builder.Result;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.io.IOException;

@RestController
@RequestMapping("/v1/configuration/eventloop")
public class EventLoop {

    @PostMapping(consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> applyConfiguration(@RequestBody EventLoopConfiguration eventLoop) throws IOException {
        eventLoop.save();

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
        String eventLoop = ConfigurationMarshaller.get(EventLoopConfiguration.DEFAULT);

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("EventLoopConfiguration").withMessage(eventLoop).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }

    @GetMapping(produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getConfiguration() {
        String eventLoop;
        try {
            EventLoopConfiguration eventLoopConfiguration = EventLoopConfiguration.load();
            eventLoop = ConfigurationMarshaller.get(eventLoopConfiguration);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.CONFIGURATION_NOT_FOUND, ex.getMessage(), HttpResponseStatus.NOT_FOUND);
        }

        APIResponse apiResponse = APIResponse.newBuilder()
                .isSuccess(true)
                .withResult(Result.newBuilder().withHeader("EventLoopConfiguration").withMessage(eventLoop).build())
                .build();

        return FastBuilder.response(apiResponse.getResponse(), HttpResponseStatus.OK);
    }
}
