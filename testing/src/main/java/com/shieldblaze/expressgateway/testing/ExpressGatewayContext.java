package com.shieldblaze.expressgateway.testing;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonRootName;

@JsonRootName("contexts")
public class ExpressGatewayContext {

    @JsonProperty
    private String restApiIpAddress;

    @JsonProperty
    private String restApiPort;
}
