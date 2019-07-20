module Update exposing (update, loadCurrentPage)

import Browser
import Messages exposing (..)
import Model exposing (..)
import Browser.Navigation as Nav
import Ports
import Route exposing (Route)
import Url
import Json.Decode as Decode


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        OnUrlRequest urlRequest ->
            case urlRequest of
                Browser.Internal url ->
                    ( model
                    , Nav.pushUrl model.navKey (Url.toString url)
                    )

                Browser.External url ->
                    ( model
                    , Nav.load url
                    )

        OnUrlChange url ->
            let
                newRoute =
                    Route.fromUrl url
            in
            { model | route = newRoute }
                |> loadCurrentPage

        ScrollHandle pageYOffset ->
            case model.route of
                Route.Home ->
                    if pageYOffset > 600 then
                        update (Fadein Section1) model
                    else if pageYOffset > 200 then
                        update (Fadein GetStart) model
                    else
                        ( model, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        Fadein view ->
            let
                asIn =
                    case view of
                        GetStart ->
                            asGetStartIn

                        Section1 ->
                            asSection1In

                newModel =
                    True
                        |> asIn model.isFadein
                        |> asIsFadein model
            in
            ( newModel, Cmd.none )

        OnWidthHandle width ->
            ( { model | width = width }, Cmd.none )

        OnSearchInput searchInput ->
            ( { model | searchInput = searchInput }, Cmd.none )

        Search key ->
            if key == 13 then
                -- Enter key
                ( model, Route.replaceUrl model.navKey Route.PackageList )
            else
                ( model, Cmd.none )

        HandleChecked bool ->
            ( { model | isChecked = bool }, Cmd.none )

        FetchOwnPackages (Just ownPackages) ->
            ( { model | ownPackages = Success (List.map decodePackage ownPackages) }, Cmd.none )
        FetchOwnPackages Nothing ->
            ( { model | ownPackages = Failure }, Cmd.none )

        FetchPackageVersions (Just packageVersions) ->
            ( { model | packageVersions = Success packageVersions }, Cmd.none )
        FetchPackageVersions Nothing ->
            ( { model | packageVersions = Failure }, Cmd.none )

        FetchPackage (Just package) ->
            ( { model | package = decodePackage package }, Cmd.none )
        FetchPackage Nothing ->
            ( { model | package = Failure }, Cmd.none )

        FetchReadme (Ok readme) ->
            ( { model | readme = Just readme }, Cmd.none )
        FetchReadme (Err _) ->
            ( model, Cmd.none )


decodePackage : String -> RemoteData PackageMetadata
decodePackage json =
    case Decode.decodeString packageDecoder json of
        Ok value ->
            Success value
        Err _ ->
            Failure


setSection1 : Bool -> IsFadein -> IsFadein
setSection1 newBool isFadein =
    { isFadein | section1 = newBool }


asSection1In : IsFadein -> Bool -> IsFadein
asSection1In =
    \b a -> setSection1 a b


setGetStart : Bool -> IsFadein -> IsFadein
setGetStart newBool isFadein =
    { isFadein | getStart = newBool }


asGetStartIn : IsFadein -> Bool -> IsFadein
asGetStartIn =
    \b a -> setGetStart a b


setIsFadein : IsFadein -> Model -> Model
setIsFadein newIsFadein model =
    { model | isFadein = newIsFadein }


asIsFadein : Model -> IsFadein -> Model
asIsFadein =
    \b a -> setIsFadein a b


loadCurrentPage : Model -> ( Model, Cmd Msg )
loadCurrentPage model =
    case model.route of
        Route.Home ->
            ( model, Ports.suggest () )

        Route.PackageList ->
            ( model, Ports.instantsearch () )

        Route.OwnPackages owner ->
            case model.ownPackages of
                NotRequested ->
                    ( { model | ownPackages = Requesting }
                    , Ports.fetchOwnPackages (owner)
                    )

                _ ->
                    ( model, Cmd.none )

        Route.PackageVersions owner repo ->
            case model.packageVersions of
                NotRequested ->
                    ( { model | packageVersions = Requesting }
                    , Ports.fetchPackageVersions (owner, repo)
                    )

                _ ->
                    ( model, Cmd.none )

        Route.Package owner repo version ->
            case model.package of
                NotRequested ->
                    ( { model | package = Requesting }
                    , Ports.fetchPackage (owner, repo, version)
                    )

                Success packageMetadata ->
                    if packageMetadata.owner /= owner
                    && packageMetadata.repo /= repo then
                        ( { model | package = Requesting }
                        , Ports.fetchPackage (owner, repo, version)
                        )

                    else
                        ( model, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        _ ->
            ( model, Cmd.none )
