module Update exposing (update, urlUpdate)

import Commands exposing (..)
import Messages exposing (..)
import Model exposing (..)
import Navigation
import Routing exposing (Route(..), parse, toPath)
import Uuid exposing (uuidGenerator)
import Random.Pcg exposing (step)


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UrlChange location ->
            let
                currentRoute =
                    parse location
            in
                urlUpdate { model | route = currentRoute }

        NavigateTo route ->
            model ! [ Navigation.newUrl <| toPath route ]

        AutoLogin ->
            model ! [ Navigation.load "/auth" ]

        HandleSearchInput value ->
            { model | search = value } ! []

        HandleTokenInput value ->
            { model | tokenName = value } ! []

        LoginUserResult (Ok response) ->
            { model | loginUser = Success response } ! []
        LoginUserResult (Err error) ->
            { model | loginUser = Failure (toString error) } ! []

        OtherUserResult (Ok response) ->
            { model | otherUser = Success response } ! []
        OtherUserResult (Err error) ->
            { model | otherUser = Failure (toString error) } ! []

        DeleteSession ->
            model ! [ logout model.csrfToken ]

        PostDeleted (Ok response) ->
            { model | loginUser = NotRequested } ! []
        PostDeleted (Err error) ->
            model ! [ Debug.crash (toString error) ]

        SelectMeta string ->
            { model | csrfToken = string } ! []

        NewToken ->
            case model.tokenName of
                "" -> -- TODO: I want error
                    model ! []
                tokenName ->
                    let
                        ( newToken, newSeed ) =
                            step uuidGenerator model.currentSeed
                        newTokenList =
                            case model.loginUser of
                                Success user ->
                                    case user.token of
                                        Nothing ->
                                            Just (List.singleton (genNewToken newToken tokenName))
                                        Just uuidList ->
                                            Just (uuidList ++ [genNewToken newToken tokenName])
                                _ ->
                                    Nothing
                    in
                        { model
                            | currentSeed = newSeed
                            , tokenName = ""
                        } ! [
                            case model.loginUser of
                                Success user ->
                                    updateToken user newTokenList
                                _ ->
                                    Cmd.none
                        ]
        DeleteToken id ->
            let
                newTokenList =
                    case model.loginUser of
                        Success user ->
                            case user.token of
                                Nothing ->
                                    Nothing
                                Just uuidList ->
                                    Just (List.filter (isInclude id) uuidList)
                        _ ->
                            Nothing
            in
                 model ! [
                     case model.loginUser of
                         Success user ->
                             updateToken user newTokenList
                         _ ->
                             Cmd.none
                 ]

        TokenUpdated (Ok user) ->
            { model | loginUser = Success user } ! []
        TokenUpdated (Err error) ->
            { model | loginUser = Failure (toString error) } ! []

--        KeyDown 191 ->
--            model ! [ FocusOn ]

        _ ->
            model ! []

isInclude : String -> Token -> Bool
isInclude id token =
    token.id /= id

genNewToken : Uuid.Uuid -> String -> Token
genNewToken newToken tokenName =
    Token (Uuid.toString newToken)
          tokenName
          ""
          Nothing

urlUpdate : Model -> ( Model, Cmd Msg )
urlUpdate model =
    case model.route of
        -- TODO: Render the 404 page on request.
        -- TODO:  Because I want use Requesting.
        UsersRoute id ->
            case (model.loginUser, model.otherUser) of
                (NotRequested, NotRequested) ->
                    model ! [ getSession, getUser id ]
                (NotRequested, _) ->
                    model ! [ getSession ]
                (_, NotRequested) ->
                    model ! [ getUser id ]
                _ ->
                    model ! []

--        SettingsRoute ->
--            case model.loginUser of
--                NotRequested ->
--                    model ! [  ]
--                _ ->
--                    model ! []

        _ ->
            case model.loginUser of
                NotRequested ->
                    model ! [ getSession ]
                _ ->
                    model ! []
