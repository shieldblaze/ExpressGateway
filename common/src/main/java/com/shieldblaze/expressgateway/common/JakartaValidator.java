package com.shieldblaze.expressgateway.common;

import jakarta.validation.ConstraintViolation;
import jakarta.validation.Validation;
import jakarta.validation.Validator;
import jakarta.validation.ValidatorFactory;

import java.util.Set;

/**
 * <p> JakartaValidator is a utility class that provides a method to validate an object using Jakarta Bean Validation API. </p>
 */
public final class JakartaValidator {

    private static final ValidatorFactory factory = Validation.buildDefaultValidatorFactory();
    private static final Validator validator = factory.getValidator();

    private JakartaValidator() {
        // Prevent instantiation
    }

    /**
     * Validate an object using Jakarta Bean Validation API.
     *
     * @param object The object to validate
     * @param <T>    The type of the object
     * @throws IllegalArgumentException If the object is invalid
     */
    public static <T> void validate(T object) {
        Set<ConstraintViolation<T>> violations = validator.validate(object);
        if (!violations.isEmpty()) {
            StringBuilder errorMessage = new StringBuilder("Validation errors:");
            for (ConstraintViolation<T> violation : violations) {
                errorMessage.append("\n - ").append(new Violation(violation.getPropertyPath().toString(), violation.getMessage()));
            }
            throw new IllegalArgumentException(errorMessage.toString());
        }
    }

    /**
     * A record to represent a validation violation.
     */
    public record Violation(String propertyPath, String message) {

        @Override
        public String toString() {
            return propertyPath + ": " + message;
        }
    }
}
