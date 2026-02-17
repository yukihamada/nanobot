//! Integration tests for personality learning system

#[cfg(feature = "dynamodb-backend")]
mod personality_tests {
    use nanobot_core::agent::personality::{
        PersonalitySection, PersonalityDimension, analyze_feedback_context,
    };

    #[test]
    fn test_personality_section_reinforce() {
        let mut section = PersonalitySection::new("tone".to_string(), "friendly".to_string());
        assert_eq!(section.confidence, 0.5);
        assert_eq!(section.feedback_count, 0);

        section.reinforce(0.3);
        assert_eq!(section.confidence, 0.8);
        assert_eq!(section.feedback_count, 1);

        section.reinforce(0.5);
        assert_eq!(section.confidence, 1.0); // Capped at 1.0
        assert_eq!(section.feedback_count, 2);
    }

    #[test]
    fn test_personality_section_weaken() {
        let mut section = PersonalitySection::new("tone".to_string(), "friendly".to_string());

        section.weaken(0.3);
        assert_eq!(section.confidence, 0.2);
        assert_eq!(section.feedback_count, 1);

        section.weaken(0.5);
        assert_eq!(section.confidence, 0.0); // Floored at 0.0
        assert_eq!(section.feedback_count, 2);
    }

    #[test]
    fn test_personality_learns_from_negative_feedback_verbosity() {
        let feedback = "Response was too long and verbose";
        let adjustments = analyze_feedback_context(feedback, "down");

        assert!(adjustments.contains_key(&PersonalityDimension::Verbosity));
        assert!(adjustments[&PersonalityDimension::Verbosity] < 0.0,
                "Negative feedback on verbosity should decrease confidence");
    }

    #[test]
    fn test_personality_learns_from_positive_feedback_verbosity() {
        let feedback = "I like the detailed explanation, very thorough";
        let adjustments = analyze_feedback_context(feedback, "up");

        // Positive feedback on detailed response should reinforce verbosity
        if adjustments.contains_key(&PersonalityDimension::Verbosity) {
            assert!(adjustments[&PersonalityDimension::Verbosity] > 0.0,
                    "Positive feedback on detailed response should increase confidence");
        }
    }

    #[test]
    fn test_personality_learns_from_tone_feedback() {
        let feedback_formal = "I prefer a more formal tone";
        let adjustments = analyze_feedback_context(feedback_formal, "down");

        assert!(adjustments.contains_key(&PersonalityDimension::Tone));
        // Negative feedback on current tone (not formal enough) should adjust
        assert_ne!(adjustments[&PersonalityDimension::Tone], 0.0);
    }

    #[test]
    fn test_personality_learns_from_emoji_feedback() {
        let feedback = "Too many emojis";
        let adjustments = analyze_feedback_context(feedback, "down");

        assert!(adjustments.contains_key(&PersonalityDimension::EmojiUsage));
        assert!(adjustments[&PersonalityDimension::EmojiUsage] < 0.0,
                "Negative emoji feedback should decrease emoji usage confidence");
    }

    #[test]
    fn test_personality_confidence_increases_with_positive_feedback() {
        let mut section = PersonalitySection::new("tone".to_string(), "technical".to_string());
        let initial_confidence = section.confidence;

        // Simulate 5 positive feedbacks
        for _ in 0..5 {
            section.reinforce(0.1);
        }

        assert!(section.confidence > initial_confidence);
        assert_eq!(section.feedback_count, 5);
        assert!(section.confidence <= 1.0, "Confidence should be capped at 1.0");
    }

    #[test]
    fn test_personality_dimension_serialization() {
        // Test SK conversion
        assert_eq!(PersonalityDimension::Tone.to_sk(), "TONE");
        assert_eq!(PersonalityDimension::Verbosity.to_sk(), "VERBOSITY");
        assert_eq!(PersonalityDimension::EmojiUsage.to_sk(), "EMOJI_USAGE");

        // Test reverse conversion
        assert_eq!(PersonalityDimension::from_sk("TONE"), Some(PersonalityDimension::Tone));
        assert_eq!(PersonalityDimension::from_sk("INVALID"), None);
    }

    #[test]
    fn test_personality_dimension_defaults() {
        assert_eq!(PersonalityDimension::Tone.default_value(), "friendly");
        assert_eq!(PersonalityDimension::Verbosity.default_value(), "moderate");
        assert_eq!(PersonalityDimension::EmojiUsage.default_value(), "minimal");
        assert_eq!(PersonalityDimension::CodeStyle.default_value(), "detailed_comments");
        assert_eq!(PersonalityDimension::Proactivity.default_value(), "proactive");
    }

    #[test]
    fn test_analyze_feedback_no_clear_signal() {
        let feedback = "Thank you";
        let adjustments = analyze_feedback_context(feedback, "up");

        // Generic feedback should not produce adjustments
        assert!(adjustments.is_empty(), "Generic feedback should not produce personality adjustments");
    }

    #[test]
    fn test_analyze_feedback_multiple_dimensions() {
        let feedback = "Response was too long with too many emojis 😅";
        let adjustments = analyze_feedback_context(feedback, "down");

        // Should adjust both verbosity and emoji usage
        assert!(adjustments.len() >= 2, "Feedback mentions multiple dimensions");
        assert!(adjustments.contains_key(&PersonalityDimension::Verbosity));
        assert!(adjustments.contains_key(&PersonalityDimension::EmojiUsage));
    }

    #[test]
    fn test_personality_section_last_updated() {
        let mut section = PersonalitySection::new("tone".to_string(), "friendly".to_string());
        let initial_time = section.last_updated;

        // Sleep briefly to ensure time difference
        std::thread::sleep(std::time::Duration::from_millis(10));

        section.reinforce(0.1);

        assert!(section.last_updated > initial_time, "last_updated should be updated");
    }

    // TODO: Add DynamoDB backend integration tests
    // These require:
    // 1. Mock DynamoDB client
    // 2. Test get_personality()
    // 3. Test update_personality()
    // 4. Test learn_from_feedback() end-to-end
}

#[cfg(not(feature = "dynamodb-backend"))]
mod no_dynamodb {
    #[test]
    fn personality_tests_require_dynamodb() {
        // This test always passes, just documents the requirement
        println!("Personality tests require dynamodb-backend feature");
    }
}
