module Contact.View exposing (..)

import Common.View exposing (warningMessage, backToHomeLink)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Routing exposing (Route(..))


contactView : Contact -> ( String, Html Msg )
contactView model =
    let
        classes =
            classList
                [ ( "card", True )
                , ( "male", model.gender == 0 )
                , ( "female", model.gender == 1 )
                ]

        fullName =
            model.first_name ++ " " ++ model.last_name
    in
        ( toString model.id
        , div
            [ classes
            , onClick <| NavigateTo <| ShowContactRoute model.id
            ]
            [ div
                [ class "inner" ]
                [ header
                    []
                    [ div
                        [ class "avatar-wrapper" ]
                        [ img
                            [ class "avatar"
                            , src model.picture
                            ]
                            []
                        ]
                    , div
                        [ class "info-wrapper" ]
                        [ h4
                            []
                            [ text fullName ]
                        , ul
                            [ class "meta" ]
                            [ li
                                []
                                [ i
                                    [ class "fa fa-map-marker" ]
                                    []
                                , text model.location
                                ]
                            , li
                                []
                                [ i
                                    [ class "fa fa-birthday-cake" ]
                                    []
                                , text model.birth_date
                                ]
                            ]
                        ]
                    ]
                , div
                    [ class "card-body" ]
                    [ div
                        [ class "headline" ]
                        [ p [] [ text model.headline ] ]
                    , ul
                        [ class "contact-info" ]
                        [ li
                            []
                            [ i
                                [ class "fa fa-phone" ]
                                []
                            , text model.phone_number
                            ]
                        , li
                            []
                            [ i
                                [ class "fa fa-envelope" ]
                                []
                            , text model.email
                            ]
                        ]
                    ]
                ]
            ]
        )


showContactView : Model -> Html Msg
showContactView model =
    case model.contact of
        Success contact ->
            let
                classes =
                    classList
                        [ ( "person-detail", True )
                        , ( "male", contact.gender == 0 )
                        , ( "female", contact.gender == 1 )
                        ]

                ( _, content ) =
                    contactView contact
            in
                div
                    [ id "contacts_show" ]
                    [ header []
                        [ h3
                            []
                            [ text "Person detail" ]
                        ]
                    , backToHomeLink
                    , div
                        [ classes ]
                        [ content ]
                    ]

        Requesting ->
            warningMessage
                "fa fa-spin fa-cog fa-2x fa-fw"
                "Fetching contact"
                (text "")

        Failure error ->
            warningMessage
                "fa fa-meh-o fa-stack-2x"
                error
                backToHomeLink

        NotRequested ->
            text ""
